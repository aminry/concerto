//! Manual smoke-test for the typed keychain wrapper.
//!
//! Usage:
//!     cargo run -p concerto-keychain --example secrets_demo -- set <token>
//!     cargo run -p concerto-keychain --example secrets_demo -- get
//!     cargo run -p concerto-keychain --example secrets_demo -- delete
//!
//! Operates on the real `concerto` service in your OS keychain so that
//! you can confirm the entry appears in Keychain Access (macOS) under
//! service=`concerto`, account=`provider_token.anthropic`. macOS will
//! prompt the first time you call `get` for the entry; subsequent calls
//! are silent for the user session.
//!
//! This is a development convenience — not part of any shipped binary.

use concerto_keychain::{Provider, SecretKind, SecretValue, Secrets};

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 2 {
        eprintln!("usage: secrets_demo <set <value>|get|delete>");
        std::process::exit(2);
    }
    let secrets = Secrets::new();
    let kind = SecretKind::ProviderToken(Provider::Anthropic);

    match args[1].as_str() {
        "set" => {
            let value = args
                .get(2)
                .ok_or("`set` requires a value argument")?
                .clone();
            secrets.set(kind, SecretValue::new(value)).await?;
            println!("ok: stored ProviderToken(Anthropic)");
        }
        "get" => match secrets.get(kind).await? {
            Some(v) => println!("ok: got {} bytes", v.expose().len()),
            None => println!("none: no entry for ProviderToken(Anthropic)"),
        },
        "delete" => {
            secrets.delete(kind).await?;
            println!("ok: deleted ProviderToken(Anthropic)");
        }
        other => {
            eprintln!("unknown subcommand: {other}");
            std::process::exit(2);
        }
    }
    Ok(())
}
