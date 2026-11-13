//! Helper: rerout-injector.exe <pid> <путь к dll>
//! Собирается под x86 и x64; внедряет DLL в процесс своей разрядности.
use std::path::Path;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 3 {
        eprintln!("usage: rerout-injector <pid> <dll>");
        std::process::exit(2);
    }
    let Ok(pid) = args[1].parse::<u32>() else {
        eprintln!("bad pid");
        std::process::exit(2);
    };
    match rerout_inject::inject_same_arch(pid, Path::new(&args[2])) {
        Ok(()) => {}
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(1);
        }
    }
}
