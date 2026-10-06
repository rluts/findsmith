use parsoid::prelude::*;

use crate::{
    projects::{ReplacementRule, RuleOperation},
    rules::{
        MAX_INPUT_BYTES, MAX_OUTPUT_BYTES, MAX_TOTAL_REPLACEMENTS, RuleEngine, TransformationResult,
    },
};

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
        let enabled = rule.enabled
            && (rule.group_name.is_empty()
                || input.contains(&rule.group_contains) != rule.group_invert);
        if !enabled {
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
            let mut standalone = rule.clone();
            standalone.group_name.clear();
            standalone.group_contains.clear();
            standalone.group_invert = false;
            let result = RuleEngine::compile_with_category_prefix(&[standalone], category_prefix)
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
    if rule.operation == RuleOperation::CategoryReplace
        && category_name(&rule.find)
            .replace('_', " ")
            .eq_ignore_ascii_case(&category_name(&rule.replace).replace('_', " "))
    {
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
                .any(|item| same_category(&item.category(), &rule.replace))
        {
            code.append(&Category::new(
                &format!("Category:{}", category_name(&rule.replace)),
                None,
            ));
            count = 1;
        }
        for category in &categories {
            if !matches!(
                rule.operation,
                RuleOperation::CategoryRemove | RuleOperation::CategoryReplace
            ) || !same_category(&category.category(), &rule.find)
            {
                continue;
            }
            if rule.operation == RuleOperation::CategoryRemove
                || categories
                    .iter()
                    .any(|item| same_category(&item.category(), &rule.replace))
            {
                category.detach();
            } else {
                category.set_category(&format!("Category:{}", category_name(&rule.replace)));
            }
            count += 1;
        }
        for template in &templates {
            if !template.is_template() || !same_template(&template.name_in_wikitext(), &rule.find) {
                continue;
            }
            let changed = match rule.operation {
                RuleOperation::TemplateRename => {
                    let name = template_name(&rule.replace);
                    if same_template(&template.name_in_wikitext(), name) {
                        false
                    } else {
                        template
                            .set_name(name.to_owned())
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

fn same_template(left: &str, right: &str) -> bool {
    template_name(left)
        .replace('_', " ")
        .eq_ignore_ascii_case(&template_name(right).replace('_', " "))
}

fn category_name(name: &str) -> &str {
    let name = name.trim();
    match name.split_once(':') {
        Some((prefix, rest)) if prefix.eq_ignore_ascii_case("Category") => rest.trim(),
        _ => name,
    }
}

fn same_category(left: &str, right: &str) -> bool {
    let left = left.split_once(':').map(|(_, name)| name).unwrap_or(left);
    left.replace('_', " ")
        .eq_ignore_ascii_case(&category_name(right).replace('_', " "))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_match_canonical_and_underscored_forms() {
        assert!(same_template("Template:Infobox_person", "Infobox person"));
        assert!(same_category("Категорія:New_York", "New York"));
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
