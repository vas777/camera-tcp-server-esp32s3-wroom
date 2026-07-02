use std::{fs::OpenOptions, io::Write, path::Path};

fn main() {
    linker_be_nice();
    read_env();
    // make sure linkall.x is the last linker script (otherwise might cause problems with flip-link)
    println!("cargo:rustc-link-arg=-Tlinkall.x");
    println!("cargo:rerun-if-env-changed=DEFAULT_PORT");
    println!("cargo:rerun-if-env-changed=DEFAULT_RELAY_PORT");
}

fn read_env() {
    use std::str::FromStr;
    let out_dir = std::env::var("OUT_DIR").unwrap();
    let mut default_env_vars = Vec::new();
    default_env_vars.push(("DEFAULT_PORT", "8080"));
    default_env_vars.push(("DEFAULT_RELAY_PORT", "5000"));

    let port_rs = Path::new(&out_dir).join("port.rs");
    let mut file = OpenOptions::new()
        .create(true)
        .write(true)
        .open(port_rs)
        .unwrap();

    for (env, default) in default_env_vars {
        let p = std::env::var(env).unwrap_or(default.to_owned());
        let port = u16::from_str(&p).unwrap();
        file.write(format!("pub const {}_ENV: u16 = {};", env, port).as_bytes())
            .unwrap();
    }
}

fn linker_be_nice() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() > 1 {
        let kind = &args[1];
        let what = &args[2];

        match kind.as_str() {
            "undefined-symbol" => match what.as_str() {
                what if what.starts_with("_defmt_") => {
                    eprintln!();
                    eprintln!(
                        "💡 `defmt` not found - make sure `defmt.x` is added as a linker script and you have included `use defmt_rtt as _;`"
                    );
                    eprintln!();
                }
                "_stack_start" => {
                    eprintln!();
                    eprintln!("💡 Is the linker script `linkall.x` missing?");
                    eprintln!();
                }
                what if what.starts_with("esp_rtos_") => {
                    eprintln!();
                    eprintln!(
                        "💡 `esp-radio` has no scheduler enabled. Make sure you have initialized `esp-rtos` or provided an external scheduler."
                    );
                    eprintln!();
                }
                "embedded_test_linker_file_not_added_to_rustflags" => {
                    eprintln!();
                    eprintln!(
                        "💡 `embedded-test` not found - make sure `embedded-test.x` is added as a linker script for tests"
                    );
                    eprintln!();
                }
                "free"
                | "malloc"
                | "calloc"
                | "get_free_internal_heap_size"
                | "malloc_internal"
                | "realloc_internal"
                | "calloc_internal"
                | "free_internal" => {
                    eprintln!();
                    eprintln!(
                        "💡 Did you forget the `esp-alloc` dependency or didn't enable the `compat` feature on it?"
                    );
                    eprintln!();
                }
                _ => (),
            },
            // we don't have anything helpful for "missing-lib" yet
            _ => {
                std::process::exit(1);
            }
        }

        std::process::exit(0);
    }

    println!(
        "cargo:rustc-link-arg=-Wl,--error-handling-script={}",
        std::env::current_exe().unwrap().display()
    );
}
