//! A stand-in for winget in the tests. It prints its arguments, then plays the package it was asked for:
//! `astral-sh.uv` installs, `Git.Git` fails, `Python.Python.3.12` never finishes.

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    println!("ARGS {}", args.join(" "));
    let id = args.windows(2).find(|pair| pair[0] == "--id").map(|pair| pair[1].clone()).unwrap_or_default();
    match id.as_str() {
        "astral-sh.uv" => {
            println!("Found uv [astral-sh.uv] Version 0.9.0");
            println!("\r  ██████████████  100%\r");
            println!("Successfully installed");
        }
        "Git.Git" => {
            eprintln!("Installer failed with exit code: 1603");
            std::process::exit(1);
        }
        _ => loop {
            std::thread::sleep(std::time::Duration::from_secs(1));
        },
    }
}
