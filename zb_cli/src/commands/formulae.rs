pub async fn execute(
    installer: &mut zb_io::Installer,
    versions: bool,
) -> Result<(), zb_core::Error> {
    if versions {
        let names = installer.list_formula_names().await?;
        for (name, version) in installer.formula_versions(&names).await? {
            println!("{name} {version}");
        }
    } else {
        for name in installer.list_formula_names().await? {
            println!("{name}");
        }
    }

    Ok(())
}
