use parsoid::prelude::*;

use crate::{
    localization,
    mediawiki::{TitleError, TitleNamespace, normalize_in_namespace},
    projects::{ReplacementRule, RuleOperation},
    rules::{
        MAX_INPUT_BYTES, MAX_OUTPUT_BYTES, MAX_TOTAL_REPLACEMENTS, RuleEngine, TransformationResult,
    },
};

/// The wiki's normalized title for a rule's template or category name.
async fn wiki_title(
    wiki_url: &str,
    user_agent: &str,
    name: &str,
    namespace: TitleNamespace,
) -> Result<String, String> {
    normalize_in_namespace(wiki_url, user_agent, name, namespace)
        .await
        .map_err(|error| match error {
            TitleError::Invalid => localization::message_with_args(
                "findsmith-api-structural-invalid-title",
                &[name.trim()],
            )
            .expect("English title error message exists"),
            TitleError::Request(message) => {
                format!("Could not check the template or category name on the wiki: {message}")
            }
        })
}

/// Compares titles that the wiki has already normalized, such as Parsoid's
/// link targets and [`wiki_title`] results. Case matters beyond what the wiki
/// normalized, because MediaWiki titles are case-sensitive there.
fn same_title(left: &str, right: &str) -> bool {
    left.replace('_', " ") == right.replace('_', " ")
}

pub async fn apply_with_wiki(
    rules: &[ReplacementRule],
    input: &str,
    wiki_url: &str,
    user_agent: &str,
    category_prefix: &str,
) -> Result<TransformationResult, String> {
    RuleEngine::compile_with_category_prefix(rules, category_prefix)
        .map_err(|error| error.to_string())?;
    if input.len() > MAX_INPUT_BYTES {
        return Err("The input text is too large.".to_owned());
    }
    let mut text = input.to_owned();
    let mut counts = Vec::with_capacity(rules.len());
    let mut total = 0;
    for rule in rules {
        if !rule.enabled {
            counts.push(0);
            continue;
        }
        let (next, count) = if matches!(
            rule.operation,
            RuleOperation::CategoryAdd
                | RuleOperation::CategoryRemove
                | RuleOperation::CategoryReplace
                | RuleOperation::TemplateRename
                | RuleOperation::TemplateParameterSet
                | RuleOperation::TemplateParameterRemove
        ) {
            apply_structural_rule(rule, &text, wiki_url, user_agent).await?
        } else {
            let result = RuleEngine::compile_with_category_prefix(
                std::slice::from_ref(rule),
                category_prefix,
            )
            .and_then(|engine| engine.apply_intermediate(&text))
            .map_err(|error| error.to_string())?;
            (result.text, result.total_replacements)
        };
        if next.len() > MAX_OUTPUT_BYTES {
            return Err("The transformed text is too large.".to_owned());
        }
        total += count;
        if total > MAX_TOTAL_REPLACEMENTS {
            return Err("Too many replacements.".to_owned());
        }
        text = next;
        counts.push(count);
    }
    let changed = counts.iter().map(|count| *count > 0).collect();
    Ok(TransformationResult {
        text,
        counts,
        changed,
        total_replacements: total,
    })
}

async fn apply_structural_rule(
    rule: &ReplacementRule,
    input: &str,
    wiki_url: &str,
    user_agent: &str,
) -> Result<(String, usize), String> {
    let is_category = matches!(
        rule.operation,
        RuleOperation::CategoryAdd | RuleOperation::CategoryRemove | RuleOperation::CategoryReplace
    );
    let namespace = if is_category {
        TitleNamespace::Category
    } else {
        TitleNamespace::Template
    };
    let find_title = if rule.operation == RuleOperation::CategoryAdd {
        String::new()
    } else {
        wiki_title(wiki_url, user_agent, &rule.find, namespace).await?
    };
    let replace_title = if matches!(
        rule.operation,
        RuleOperation::CategoryAdd | RuleOperation::CategoryReplace | RuleOperation::TemplateRename
    ) {
        wiki_title(wiki_url, user_agent, &rule.replace, namespace).await?
    } else {
        String::new()
    };
    if rule.operation == RuleOperation::CategoryReplace && same_title(&find_title, &replace_title) {
        return Ok((input.to_owned(), 0));
    }
    let rest_url = wiki_url
        .strip_suffix("api.php")
        .ok_or_else(|| "The selected wiki has no supported Parsoid endpoint.".to_owned())?;
    let client = ParsoidClient::new(&format!("{rest_url}rest.php"), user_agent)
        .map_err(|error| format!("Could not connect to Parsoid: {error}"))?;
    let html = client
        .transform_to_html(input)
        .await
        .map_err(|error| format!("Parsoid could not parse wikitext: {error}"))?;
    let (code, count) = {
        let code = html.into_mutable();
        let templates = if matches!(
            rule.operation,
            RuleOperation::TemplateRename
                | RuleOperation::TemplateParameterSet
                | RuleOperation::TemplateParameterRemove
        ) {
            code.filter_templates()
                .map_err(|error| format!("Could not inspect templates: {error}"))?
        } else {
            Vec::new()
        };
        let categories = if matches!(
            rule.operation,
            RuleOperation::CategoryAdd
                | RuleOperation::CategoryRemove
                | RuleOperation::CategoryReplace
        ) {
            code.filter_categories()
        } else {
            Vec::new()
        };
        let mut count = 0;
        if rule.operation == RuleOperation::CategoryAdd
            && !categories
                .iter()
                .any(|item| same_title(&item.category(), &replace_title))
        {
            code.append(&Category::new(&replace_title, None));
            count = 1;
        }
        for category in &categories {
            if !matches!(
                rule.operation,
                RuleOperation::CategoryRemove | RuleOperation::CategoryReplace
            ) || !same_title(&category.category(), &find_title)
            {
                continue;
            }
            if rule.operation == RuleOperation::CategoryRemove
                || categories
                    .iter()
                    .any(|item| same_title(&item.category(), &replace_title))
            {
                category.detach();
            } else {
                category.set_category(&replace_title);
            }
            count += 1;
        }
        for template in &templates {
            if !template.is_template() || !same_title(&template.name(), &find_title) {
                continue;
            }
            let changed = match rule.operation {
                RuleOperation::TemplateRename => {
                    if same_title(&template.name(), &replace_title) {
                        false
                    } else {
                        template
                            .set_name(template_name(&rule.replace).to_owned())
                            .map_err(|error| error.to_string())?;
                        true
                    }
                }
                RuleOperation::TemplateParameterSet => {
                    if template.param(&rule.parameter).as_deref() == Some(rule.replace.as_str()) {
                        false
                    } else {
                        template
                            .set_param(&rule.parameter, &rule.replace)
                            .map_err(|error| error.to_string())?;
                        true
                    }
                }
                RuleOperation::TemplateParameterRemove => template
                    .remove_param(&rule.parameter)
                    .map_err(|error| error.to_string())?
                    .is_some(),
                _ => unreachable!(),
            };
            if changed {
                count += 1;
            }
            if changed && !rule.replace_all {
                break;
            }
        }
        drop(templates);
        drop(categories);
        (code.into_immutable(), count)
    };
    if count == 0 {
        return Ok((input.to_owned(), 0));
    }
    let output = client
        .transform_to_wikitext(&code)
        .await
        .map_err(|error| format!("Parsoid could not serialize wikitext: {error}"))?;
    if output == input {
        Ok((input.to_owned(), 0))
    } else {
        Ok((output, count))
    }
}

fn template_name(name: &str) -> &str {
    let name = name.trim();
    match name.split_once(':') {
        Some((prefix, rest)) if prefix.eq_ignore_ascii_case("Template") => rest.trim(),
        _ => name,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalized_titles_compare_case_sensitively() {
        assert!(same_title("Шаблон:Infobox_person", "Шаблон:Infobox person"));
        assert!(!same_title("Шаблон:Foo", "Шаблон:FOO"));
        assert!(!same_title("Категорія:New York", "Категорія:New york"));
    }

    #[tokio::test]
    async fn plain_rules_keep_order_without_parsoid_requests() {
        let first = ReplacementRule {
            find: "a".to_owned(),
            replace: "b".to_owned(),
            ..ReplacementRule::default()
        };
        let second = ReplacementRule {
            find: "b".to_owned(),
            replace: "c".to_owned(),
            ..ReplacementRule::default()
        };
        let result = apply_with_wiki(
            &[first, second],
            "a",
            "https://example.org/w/api.php",
            "FindSmith test",
            "Category",
        )
        .await
        .unwrap();
        assert_eq!(result.text, "c");
        assert_eq!(result.counts, vec![1, 1]);
    }
}
