//! Helper (built for x86 and x64; works on processes of its own bitness):
//!   rerout-injector <pid> <dll>     inject the hook DLL
//!   rerout-injector eject <pid>     release the hook DLL (exit code: 0 released, 10 not loaded,
//!                                   11 busy, 12 older DLL version, 1 error)
use std::path::Path;

fn main() {
    let args: Vec<String> = std::env::args().collect();

    if args.len() >= 3 && args[1] == "eject" {
        let Ok(pid) = args[2].parse::<u32>() else {
            eprintln!("bad pid");
            std::process::exit(2);
        };
        let code = match rerout_inject::eject_same_arch(pid) {
            Ok(rerout_inject::Eject::Released) => 0,
            Ok(rerout_inject::Eject::NotLoaded) => 10,
            Ok(rerout_inject::Eject::Busy) => 11,
            Ok(rerout_inject::Eject::Unsupported) => 12,
            Err(e) => {
                eprintln!("{e}");
                1
            }
        };
        std::process::exit(code);
    }

    if args.len() < 3 {
        eprintln!("usage: rerout-injector <pid> <dll> | rerout-injector eject <pid>");
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
