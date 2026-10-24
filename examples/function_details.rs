//! Prints the address ranges, inlined calls and source locations of the function
//! at each text symbol in an executable.
//!
//! This finds functions through the symbol table, so it skips functions which
//! have debug info but no symbol, for example in a stripped executable.
//!
//! Usage: cargo run --example function_details --features loader -- <path>
use object::{Object, ObjectSymbol, SymbolKind};

fn main() {
    let path = std::env::args()
        .nth(1)
        .expect("usage: function_details <path>");
    let loader = addr2line::Loader::new(&path).unwrap();
    let data = std::fs::read(&path).unwrap();
    let file = object::File::parse(&*data).unwrap();
    let mut addresses: Vec<u64> = file
        .symbols()
        .filter(|symbol| symbol.kind() == SymbolKind::Text && symbol.is_definition())
        .map(|symbol| symbol.address())
        .collect();
    addresses.sort_unstable();
    addresses.dedup();
    let mut previous_ranges = Vec::new();
    for address in addresses {
        let Some(function) = loader.find_function_details(address).unwrap() else {
            continue;
        };
        if function.ranges == previous_ranges {
            // Another symbol in the same function.
            continue;
        }
        previous_ranges = function.ranges.clone();
        let name = function.name.as_ref().map(|name| name.raw_name().unwrap());
        println!("function {:?}", name.as_deref().unwrap_or("<unknown>"));
        for range in &function.ranges {
            println!("  range {:#x}..{:#x}", range.begin, range.end);
        }
        for (index, inlined) in function.inlined_functions.iter().enumerate() {
            let name = inlined.name.as_ref().map(|name| name.raw_name().unwrap());
            println!(
                "  inlined #{index} parent {:?} depth {} {:?} called from {}:{}",
                inlined.parent,
                inlined.call_depth,
                name.as_deref().unwrap_or("<unknown>"),
                inlined.call_location.file.unwrap_or("?"),
                inlined.call_location.line.unwrap_or(0),
            );
            for range in &inlined.ranges {
                println!("    range {:#x}..{:#x}", range.begin, range.end);
            }
        }
        for row in &function.lines {
            println!(
                "  line {:#x} +{:#x} {}:{}",
                row.address,
                row.size,
                row.location.file.unwrap_or("?"),
                row.location.line.unwrap_or(0)
            );
        }
    }
}
