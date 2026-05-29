pub async fn execute(
    installer: &mut zb_io::Installer,
    versions: bool,
) -> Result<(), zb_core::Error> {
    if versions {
        for (name, version) in installer.list_formula_versions().await? {
            println!("{name} {version}");
        }
    } else {
        for name in installer.list_formula_names().await? {
            println!("{name}");
        }
    }

    Ok(())
}
