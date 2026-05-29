pub async fn execute(
    installer: &mut zb_io::Installer,
    formulas: Vec<String>,
) -> Result<(), zb_core::Error> {
    if formulas.is_empty() {
        println!("https://brew.sh/");
        return Ok(());
    }

    for formula in formulas {
        let formula = installer.formula_metadata(&formula).await?;
        let homepage = formula
            .source_url()
            .map(|url| url.url.as_str())
            .unwrap_or("https://brew.sh/");
        println!("{homepage}");
    }
    Ok(())
}
