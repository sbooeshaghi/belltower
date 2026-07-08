use bt_core::{BelltowerConfig, ConnectionModelSelector, PricingEntry, UnpricedModelEntry};

#[test]
fn pricing_catalog_covers_all_configured_models() {
    let config = BelltowerConfig::from_embedded().expect("provider catalog should load");
    let pricing_catalog = BelltowerConfig::pricing_catalog().expect("pricing catalog should load");
    let mut uncategorized = Vec::new();

    for connection in &config.connections {
        let mut models = vec![connection.default_model.clone()];
        models.extend(connection.model_fallbacks.iter().cloned());
        models.extend(
            connection
                .discoverable_model_selectors
                .iter()
                .map(selector_value),
        );
        models.sort();
        models.dedup();

        for model in models {
            let priced = best_pricing_entry(&pricing_catalog.pricing, &connection.provider, &model);
            let unpriced = best_unpriced_entry(
                &pricing_catalog.unpriced_models,
                &connection.provider,
                &model,
            );
            match (priced, unpriced) {
                (None, None) => uncategorized.push(format!("{}:{model}", connection.provider)),
                (Some(priced), Some(unpriced)) => {
                    uncategorized.push(format!(
                        "{}:{model} is both priced by `{}` and unpriced by `{}`",
                        connection.provider, priced.model_pattern, unpriced.model_pattern
                    ));
                }
                _ => {}
            }
        }
    }

    assert!(
        uncategorized.is_empty(),
        "configured models must be priced or explicitly unpriced:\n{}",
        uncategorized.join("\n")
    );
}

#[test]
fn unpriced_models_explain_why_pricing_is_absent() {
    let pricing_catalog = BelltowerConfig::pricing_catalog().expect("pricing catalog should load");
    let missing_reason_entries = pricing_catalog
        .unpriced_models
        .iter()
        .filter(|entry| entry.reason.trim().is_empty())
        .map(|entry| format!("{}:{}", entry.provider, entry.model_pattern))
        .collect::<Vec<_>>();

    assert!(
        missing_reason_entries.is_empty(),
        "unpriced models must carry reason strings:\n{}",
        missing_reason_entries.join("\n")
    );
}

fn selector_value(selector: &ConnectionModelSelector) -> String {
    match selector {
        ConnectionModelSelector::Exact { value } | ConnectionModelSelector::Prefix { value } => {
            value.clone()
        }
    }
}

fn best_pricing_entry<'a>(
    entries: &'a [PricingEntry],
    provider: &str,
    model: &str,
) -> Option<&'a PricingEntry> {
    entries
        .iter()
        .filter(|entry| entry.provider == provider && model.starts_with(&entry.model_pattern))
        .max_by_key(|entry| entry.model_pattern.len())
}

fn best_unpriced_entry<'a>(
    entries: &'a [UnpricedModelEntry],
    provider: &str,
    model: &str,
) -> Option<&'a UnpricedModelEntry> {
    entries
        .iter()
        .filter(|entry| entry.provider == provider && model.starts_with(&entry.model_pattern))
        .max_by_key(|entry| entry.model_pattern.len())
}
