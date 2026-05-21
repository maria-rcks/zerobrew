pub async fn execute(installer: &mut zb_io::Installer) -> Result<(), zb_core::Error> {
    for keg in installer.list_leaves().await? {
        println!("{}", keg.name);
    }

    Ok(())
}
