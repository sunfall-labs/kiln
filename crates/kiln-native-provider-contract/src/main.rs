use std::path::PathBuf;

fn main() {
    let mut args = std::env::args().skip(1);
    match args.next().as_deref() {
        Some("source-digest") => {
            if args.len() > 1 {
                eprintln!("source-digest accepts exactly one root");
                std::process::exit(2);
            }
            let root = args.next().map(PathBuf::from).unwrap_or_else(|| {
                eprintln!("source-digest requires an explicit repository root");
                std::process::exit(2);
            });
            match kiln_native_provider_contract::source_digest(&root) {
                Ok(digest) => println!("{}", digest.to_hex()),
                Err(error) => {
                    eprintln!("source digest failed for {}: {error}", root.display());
                    std::process::exit(1);
                }
            }
        }
        Some("contract") if args.next().is_none() => println!(
            "{}",
            serde_json::to_string(&kiln_native_provider_contract::contract()).unwrap()
        ),
        Some("abi") if args.next().is_none() => {
            println!("{}", kiln_native_provider_contract::contract().provider_abi)
        }
        Some("exports") if args.next().is_none() => println!(
            "{}",
            kiln_native_provider_contract::contract().exports.join(" ")
        ),
        _ => {
            eprintln!(
                "usage: kiln-native-provider-contract <contract|abi|exports|source-digest ROOT>"
            );
            std::process::exit(2);
        }
    }
}
