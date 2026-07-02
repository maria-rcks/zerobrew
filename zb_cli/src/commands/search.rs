use crate::ui::Ui;

pub struct SearchRequest {
    pub text: Vec<String>,
    pub formula: bool,
    pub cask: bool,
    pub name: bool,
    pub all: bool,
    pub desc: bool,
}

pub async fn execute(
    installer: &mut zb_io::Installer,
    request: SearchRequest,
    ui: &mut Ui,
) -> Result<(), zb_core::Error> {
    if request.cask && !request.formula {
        return Err(zb_core::Error::UnsupportedFormula {
            name: request.text.join(" "),
            reason: "cask search is not supported yet".to_string(),
        });
    }

    let query = request.text.join(" ");
    let results = if request.name || request.all || request.desc {
        installer
            .search_formula_index(&query, request.name && !request.all && !request.desc)
            .await?
    } else {
        installer.suggest_formulas(&query, 20).await?
    };

    if results.is_empty() {
        return Err(zb_core::Error::MissingFormula { name: query });
    }

    ui.heading("Formulae");
    for result in results {
        ui.data(result);
    }

    Ok(())
}
