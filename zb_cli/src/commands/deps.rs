use console::style;

pub async fn execute(
    installer: &mut zb_io::Installer,
    formulas: Vec<String>,
    include_build: bool,
) -> Result<(), zb_core::Error> {
    let multiple = formulas.len() > 1;
    for formula in formulas {
        let dependencies = installer
            .formula_dependencies(&formula, include_build)
            .await?;
        if multiple {
            println!("{}", style(&formula).bold());
        }
        for dependency in dependencies {
            println!("{dependency}");
        }
    }

    Ok(())
}
