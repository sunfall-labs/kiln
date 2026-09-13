use std::path::PathBuf;

fn main() {
    let mut args = std::env::args().skip(1);
    match args.next().as_deref() {
        Some("source-digest") => {
            let root = args
                .next()
                .map(PathBuf::from)
                .unwrap_or_else(kiln_native_provider_contract::repository_root);
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
        _ => {
            eprintln!("usage: kiln-native-provider-contract <contract|source-digest [root]>");
            std::process::exit(2);
        }
    }
}
