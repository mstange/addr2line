//! APIs for getting the address ranges, inlined calls and source locations of a
//! whole function, as opposed to the frames for a single address.
//!
//! These are useful for converting DWARF into other formats, for example into
//! Breakpad .sym files.
//!
use alloc::vec::Vec;

use crate::function::{Function, Functions};
use crate::line::Lines;
use crate::unit::ResUnit;
use crate::{Context, DebugFile, Error, FunctionName, Location, LookupContinuation, LookupResult};

/// The address ranges, inlined calls and source locations of a single function
/// (`DW_TAG_subprogram`).
pub struct FunctionDetails<'ctx, R: gimli::Reader> {
    /// The offset of the function's DIE within its unit.
    pub dw_die_offset: gimli::UnitOffset<R::Offset>,
    /// The function name. This is the linkage name if available.
    pub name: Option<FunctionName<R>>,
    /// The address ranges of the function, sorted by address.
    pub ranges: Vec<gimli::Range>,
    /// All inlined calls in this function, in DWARF order (depth-first pre-order),
    /// so parents always come before their children.
    pub inlined_functions: Vec<InlinedFunctionDetails<'ctx, R>>,
    /// The line table rows which cover this function's address ranges, sorted by
    /// address. The rows are clipped to the function's ranges.
    ///
    /// These rows describe the innermost source location for each address, i.e.
    /// the location inside the most deeply inlined function at that address.
    pub lines: Vec<LineRowDetails<'ctx>>,
}

/// An inlined function call (`DW_TAG_inlined_subroutine`).
pub struct InlinedFunctionDetails<'ctx, R: gimli::Reader> {
    /// The offset of the inlined subroutine's DIE within its unit.
    pub dw_die_offset: gimli::UnitOffset<R::Offset>,
    /// The name of the inlined function. This is the linkage name if available.
    pub name: Option<FunctionName<R>>,
    /// The index of the enclosing inlined call in `FunctionDetails::inlined_functions`,
    /// or `None` if the call is directly inside the outer function.
    pub parent: Option<usize>,
    /// The inline nesting depth. Calls from the outer function have depth 0.
    pub call_depth: usize,
    /// The source location of the call site.
    ///
    /// This location is in the caller: the outer function for calls with depth 0,
    /// otherwise the inlined function at index `parent`. This differs from
    /// [`Frame::location`](crate::Frame::location) of an inlined frame, which is a
    /// location inside the inlined function itself.
    pub call_location: Location<'ctx>,
    /// The address ranges of the inlined code, sorted by address.
    pub ranges: Vec<gimli::Range>,
}

/// A row of the line table, covering `size` bytes starting at `address`.
pub struct LineRowDetails<'ctx> {
    /// The start address.
    pub address: u64,
    /// The number of bytes covered by this row.
    pub size: u64,
    /// The source location.
    pub location: Location<'ctx>,
}

/// Push the rows of `lines` which overlap `begin..end`, clipped to that range.
fn push_clipped_rows<'a>(
    rows: &mut Vec<LineRowDetails<'a>>,
    lines: &'a Lines,
    begin: u64,
    end: u64,
) -> Result<(), Error> {
    for (address, size, location) in lines.find_location_range(begin, end)? {
        let start = address.max(begin);
        let row_end = address.saturating_add(size).min(end);
        if start < row_end {
            rows.push(LineRowDetails {
                address: start,
                size: row_end - start,
                location,
            });
        }
    }
    Ok(())
}

impl<R: gimli::Reader> ResUnit<R> {
    pub(crate) fn find_function_details<'unit, 'ctx: 'unit>(
        &'unit self,
        probe: u64,
        ctx: &'ctx Context<R>,
    ) -> LookupResult<
        impl LookupContinuation<Output = Result<Option<FunctionDetails<'unit, R>>, Error>, Buf = R>,
    > {
        self.dwarf_and_unit(ctx).map(move |r| {
            let (file, dw_unit) = r?;
            let functions = self.functions.borrow(dw_unit)?;
            match functions.find_address(probe) {
                Some(address) => {
                    let index = functions.addresses[address].function;
                    self.function_details(ctx, file, dw_unit, functions, index)
                        .map(Some)
                }
                None => Ok(None),
            }
        })
    }

    fn function_details<'ctx>(
        &'ctx self,
        ctx: &'ctx Context<R>,
        file: DebugFile,
        dw_unit: gimli::UnitRef<'ctx, R>,
        functions: &'ctx Functions<R>,
        index: usize,
    ) -> Result<FunctionDetails<'ctx, R>, Error> {
        // Parse the function without caching it in the context, see
        // `Context::find_function_details`.
        let lazy_function = &functions.functions[index];
        let function = Function::parse(lazy_function.dw_die_offset, file, dw_unit, ctx)?;
        let function = &function;
        let ranges = functions.ranges_by_function()[index].clone();
        let lines = self.parse_lines(&ctx.sections)?;

        let mut inlined_functions: Vec<InlinedFunctionDetails<'ctx, R>> = function
            .inlined_functions
            .iter()
            .map(|inlined| InlinedFunctionDetails {
                dw_die_offset: inlined.dw_die_offset,
                name: inlined.name.clone().map(|name| FunctionName {
                    name,
                    language: self.lang,
                }),
                parent: inlined.parent,
                call_depth: inlined.call_depth,
                call_location: Location {
                    file: inlined
                        .call_file
                        .and_then(|file_index| lines.and_then(|lines| lines.file(file_index))),
                    line: (inlined.call_line != 0).then_some(inlined.call_line),
                    column: (inlined.call_column != 0).then_some(inlined.call_column),
                },
                ranges: Vec::new(),
            })
            .collect();
        for address in &*function.inlined_addresses {
            inlined_functions[address.function]
                .ranges
                .push(address.range);
        }
        for inlined in &mut inlined_functions {
            inlined.ranges.sort_unstable_by_key(|range| range.begin);
        }

        let mut rows = Vec::new();
        if let Some(lines) = lines {
            for range in &ranges {
                push_clipped_rows(&mut rows, lines, range.begin, range.end)?;
            }
        }
        rows.sort_by_key(|row| row.address);

        Ok(FunctionDetails {
            dw_die_offset: function.dw_die_offset,
            name: function.name.clone().map(|name| FunctionName {
                name,
                language: self.lang,
            }),
            ranges,
            inlined_functions,
            lines: rows,
        })
    }
}
