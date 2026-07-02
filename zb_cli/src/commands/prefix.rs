use zb_core::formula::formula_token;

use crate::ui::Ui;

pub enum PathKind {
    Prefix,
    Cellar,
}

pub fn execute(
    prefix: &std::path::Path,
    formulas: Vec<String>,
    kind: PathKind,
    ui: &mut Ui,
) -> Result<(), zb_core::Error> {
    if formulas.is_empty() {
        let path = match kind {
            PathKind::Prefix => prefix.to_path_buf(),
            PathKind::Cellar => prefix.join("Cellar"),
        };
        ui.data(path.display());
        return Ok(());
    }

    for formula in formulas {
        let token = formula_token(&formula);
        let path = match kind {
            PathKind::Prefix => prefix.join("opt").join(token),
            PathKind::Cellar => prefix.join("Cellar").join(token),
        };
        ui.data(path.display());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::formula_token;

    #[test]
    fn formula_tokens_strip_tap_prefixes_for_paths() {
        assert_eq!(formula_token("hashicorp/tap/terraform"), "terraform");
        assert_eq!(formula_token("jq"), "jq");

        let prefix = std::path::Path::new("/opt/zerobrew");
        assert_eq!(
            prefix
                .join("opt")
                .join(formula_token("hashicorp/tap/terraform")),
            std::path::Path::new("/opt/zerobrew/opt/terraform")
        );
        assert_eq!(
            prefix
                .join("Cellar")
                .join(formula_token("hashicorp/tap/terraform")),
            std::path::Path::new("/opt/zerobrew/Cellar/terraform")
        );
    }
}
