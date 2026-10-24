//! Tests for `Context::find_function_details`.

use gimli::write::{
    Address, AttributeValue, DwarfUnit, EndianVec, LineProgram, LineString, Sections,
};
use gimli::{Encoding, Format, LineEncoding, LittleEndian};

/// Builds DWARF for two functions:
///
///  - `outer` at 0x1000..0x1020, with the inlined call `a` at 0x1008..0x1010
///    (called from f.c:10), which contains the inlined call `b` at 0x100c..0x1010
///    (called from a.h:3).
///  - `other` at 0x1020..0x1030, without inlined calls.
///
/// 0x1030..0x1040 has line rows from asm.s, but no function, like code written in
/// assembly.
fn build_dwarf() -> Sections<EndianVec<LittleEndian>> {
    let encoding = Encoding {
        format: Format::Dwarf32,
        version: 4,
        address_size: 8,
    };
    let mut dwarf = DwarfUnit::new(encoding);
    let mut program = LineProgram::new(
        encoding,
        LineEncoding::default(),
        LineString::String(b"/src".to_vec()),
        None,
        LineString::String(b"f.c".to_vec()),
        None,
    );
    let dir = program.default_directory();
    let f_c = program.add_file(LineString::String(b"f.c".to_vec()), dir, None);
    let a_h = program.add_file(LineString::String(b"a.h".to_vec()), dir, None);
    let b_h = program.add_file(LineString::String(b"b.h".to_vec()), dir, None);
    let asm_s = program.add_file(LineString::String(b"asm.s".to_vec()), dir, None);
    program.begin_sequence(Some(Address::Constant(0x1000)));
    for (address_offset, file, line) in [
        (0x0, f_c, 9),
        (0x8, a_h, 2),
        (0xc, b_h, 7),
        (0x10, f_c, 11),
        (0x20, f_c, 20),
        (0x30, asm_s, 30),
        (0x38, asm_s, 31),
    ] {
        let row = program.row();
        row.address_offset = address_offset;
        row.file = file;
        row.line = line;
        program.generate_row();
    }
    program.end_sequence(0x40);
    dwarf.unit.line_program = program;

    let unit = &mut dwarf.unit;
    let root = unit.root();
    let cu = unit.get_mut(root);
    cu.set(gimli::DW_AT_name, AttributeValue::String(b"f.c".to_vec()));
    cu.set(
        gimli::DW_AT_comp_dir,
        AttributeValue::String(b"/src".to_vec()),
    );
    cu.set(
        gimli::DW_AT_low_pc,
        AttributeValue::Address(Address::Constant(0x1000)),
    );
    cu.set(gimli::DW_AT_high_pc, AttributeValue::Udata(0x40));

    let mut add_abstract = |name: &str| {
        let id = unit.add(root, gimli::DW_TAG_subprogram);
        let die = unit.get_mut(id);
        die.set(gimli::DW_AT_name, AttributeValue::String(name.into()));
        die.set(
            gimli::DW_AT_inline,
            AttributeValue::Inline(gimli::DW_INL_inlined),
        );
        id
    };
    let a = add_abstract("a");
    let b = add_abstract("b");

    let add_function = |unit: &mut gimli::write::Unit, name: &str, start: u64, size: u64| {
        let id = unit.add(root, gimli::DW_TAG_subprogram);
        let die = unit.get_mut(id);
        die.set(gimli::DW_AT_name, AttributeValue::String(name.into()));
        die.set(
            gimli::DW_AT_low_pc,
            AttributeValue::Address(Address::Constant(start)),
        );
        die.set(gimli::DW_AT_high_pc, AttributeValue::Udata(size));
        id
    };
    let add_inlined = |unit: &mut gimli::write::Unit,
                       parent,
                       origin,
                       start: u64,
                       size: u64,
                       call_file,
                       call_line: u64| {
        let id = unit.add(parent, gimli::DW_TAG_inlined_subroutine);
        let die = unit.get_mut(id);
        die.set(
            gimli::DW_AT_abstract_origin,
            AttributeValue::UnitRef(origin),
        );
        die.set(
            gimli::DW_AT_low_pc,
            AttributeValue::Address(Address::Constant(start)),
        );
        die.set(gimli::DW_AT_high_pc, AttributeValue::Udata(size));
        die.set(
            gimli::DW_AT_call_file,
            AttributeValue::FileIndex(Some(call_file)),
        );
        die.set(gimli::DW_AT_call_line, AttributeValue::Udata(call_line));
        id
    };
    let outer = add_function(unit, "outer", 0x1000, 0x20);
    let inlined_a = add_inlined(unit, outer, a, 0x1008, 0x8, f_c, 10);
    add_inlined(unit, inlined_a, b, 0x100c, 0x4, a_h, 3);
    add_function(unit, "other", 0x1020, 0x10);

    let mut sections = Sections::new(EndianVec::new(LittleEndian));
    dwarf.write(&mut sections).unwrap();
    sections
}

fn with_context(f: impl FnOnce(&addr2line::Context<gimli::EndianSlice<'_, LittleEndian>>)) {
    let sections = build_dwarf();
    let dwarf = gimli::Dwarf::load(|id| -> Result<_, gimli::Error> {
        let data = sections.get(id).map(|w| w.slice()).unwrap_or(&[]);
        Ok(gimli::EndianSlice::new(data, LittleEndian))
    })
    .unwrap();
    let ctx = addr2line::Context::from_dwarf(dwarf).unwrap();
    f(&ctx);
}

fn name<R: gimli::Reader>(name: &Option<addr2line::FunctionName<R>>) -> String {
    name.as_ref().unwrap().raw_name().unwrap().into_owned()
}

fn range(begin: u64, end: u64) -> gimli::Range {
    gimli::Range { begin, end }
}

#[test]
fn find_function_details() {
    with_context(|ctx| {
        let details = ctx
            .find_function_details(0x100d)
            .skip_all_loads()
            .unwrap()
            .unwrap();
        assert_eq!(name(&details.name), "outer");
        assert_eq!(details.ranges, vec![range(0x1000, 0x1020)]);

        let inlined: Vec<_> = details
            .inlined_functions
            .iter()
            .map(|i| {
                (
                    name(&i.name),
                    i.parent,
                    i.call_depth,
                    i.call_location.file.unwrap().to_string(),
                    i.call_location.line,
                    i.ranges.clone(),
                )
            })
            .collect();
        assert_eq!(
            inlined,
            vec![
                (
                    "a".to_string(),
                    None,
                    0,
                    "/src/f.c".to_string(),
                    Some(10),
                    vec![range(0x1008, 0x1010)]
                ),
                (
                    "b".to_string(),
                    Some(0),
                    1,
                    "/src/a.h".to_string(),
                    Some(3),
                    vec![range(0x100c, 0x1010)]
                ),
            ]
        );

        // The line rows are clipped to the function: the row at 0x1010 extends to
        // 0x1020 in the line table, where `other` starts.
        let lines: Vec<_> = details
            .lines
            .iter()
            .map(|row| {
                (
                    row.address,
                    row.size,
                    row.location.file.unwrap(),
                    row.location.line.unwrap(),
                )
            })
            .collect();
        assert_eq!(
            lines,
            vec![
                (0x1000, 0x8, "/src/f.c", 9),
                (0x1008, 0x4, "/src/a.h", 2),
                (0x100c, 0x4, "/src/b.h", 7),
                (0x1010, 0x10, "/src/f.c", 11),
            ]
        );

        let details = ctx
            .find_function_details(0x1028)
            .skip_all_loads()
            .unwrap()
            .unwrap();
        assert_eq!(name(&details.name), "other");
        assert!(details.inlined_functions.is_empty());
        assert_eq!(details.lines.len(), 1);
        assert_eq!(
            (details.lines[0].address, details.lines[0].size),
            (0x1020, 0x10)
        );

        // Assembly code has line rows but no function entry.
        assert!(ctx
            .find_function_details(0x1034)
            .skip_all_loads()
            .unwrap()
            .is_none());
        assert!(ctx
            .find_function_details(0x2000)
            .skip_all_loads()
            .unwrap()
            .is_none());
    });
}
