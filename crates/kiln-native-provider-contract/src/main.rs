use std::path::PathBuf;

fn main() {
    let mut args = std::env::args().skip(1);
    match args.next().as_deref() {
        Some("source-digest") => {
            let root = args.next().map(PathBuf::from).unwrap_or_else(|| {
                eprintln!("source-digest requires an explicit repository root");
                std::process::exit(2);
            });
            println!(
                "{}",
                kiln_native_provider_contract::source_digest(&root)
                    .expect("source digest")
                    .to_hex()
            );
        }
        Some("contract") => println!(
            "{}",
            serde_json::to_string(&kiln_native_provider_contract::contract()).unwrap()
        ),
        Some("abi") => println!("{}", kiln_native_provider_contract::contract().provider_abi),
        Some("exports") => println!(
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
