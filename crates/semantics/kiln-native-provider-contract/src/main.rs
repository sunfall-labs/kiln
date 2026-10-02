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
        Some("gui-link-arguments") => {
            let values: Vec<_> = args.collect();
            let Some(target) = values.first().filter(|_| values.len() == 1) else {
                eprintln!("gui-link-arguments requires one Rust target triple");
                std::process::exit(2);
            };
            println!(
                "{}",
                kiln_native_provider_contract::gui::link_arguments(target).join(" ")
            );
        }
        Some("variant") => {
            let values: Vec<_> = args.collect();
            let Some(variant) = values
                .first()
                .filter(|_| values.len() == 1)
                .and_then(|value| kiln_native_provider_contract::ProviderVariant::parse(value))
            else {
                eprintln!("variant requires one valid provider variant name");
                std::process::exit(2);
            };
            println!(
                "{}",
                serde_json::to_string(&kiln_native_provider_contract::package(variant)).unwrap()
            );
        }
        Some("manifest") => {
            let values: Vec<_> = args.collect();
            if values.len() != 8 {
                eprintln!(
                    "manifest requires 8 arguments: target rustc source archive deployment libc variant link-arguments"
                );
                std::process::exit(2);
            }
            let variant = kiln_native_provider_contract::ProviderVariant::parse(&values[6])
                .unwrap_or_else(|| {
                    eprintln!(
                        "manifest variant must be one of: {}",
                        kiln_native_provider_contract::ProviderVariant::ALL
                            .map(kiln_native_provider_contract::ProviderVariant::as_str)
                            .join(", ")
                    );
                    std::process::exit(2);
                });
            let m = kiln_native_provider_contract::manifest(
                values[0].clone(),
                values[1].clone(),
                values[2].clone(),
                values[3].clone(),
                values[4].clone(),
                values[5].clone(),
                variant,
                values[7].split_whitespace().map(str::to_owned).collect(),
            );
            println!("{}", serde_json::to_string_pretty(&m).unwrap());
        }
        _ => {
            eprintln!(
                "usage: kiln-native-provider-contract <contract|variant NAME|abi|exports|gui-link-arguments TARGET|manifest ...|source-digest ROOT>"
            );
            std::process::exit(2);
        }
    }
}
