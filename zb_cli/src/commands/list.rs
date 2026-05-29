use console::style;

pub fn execute(
    installer: &mut zb_io::Installer,
    formulas: Vec<String>,
    versions: bool,
    json: bool,
) -> Result<(), zb_core::Error> {
    let installed = installer.list_installed()?;
    let had_installed = !installed.is_empty();
    let installed: Vec<_> = if formulas.is_empty() {
        installed
    } else {
        installed
            .into_iter()
            .filter(|keg| formulas.iter().any(|formula| formula == &keg.name))
            .collect()
    };

    if json {
        let packages: Vec<serde_json::Value> = installed
            .iter()
            .map(|keg| {
                if versions {
                    serde_json::json!({
                        "name": keg.name,
                        "versions": [keg.version],
                    })
                } else {
                    serde_json::json!(keg.name)
                }
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&packages).unwrap());
        return Ok(());
    }

    if installed.is_empty() {
        if formulas.is_empty() && !had_installed {
            println!("No formulas installed.");
        }
    } else {
        for keg in installed {
            if versions {
                println!("{} {}", keg.name, keg.version);
            } else {
                println!("{}", style(&keg.name).bold());
            }
        }
    }

    Ok(())
}
