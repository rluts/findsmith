use std::collections::HashSet;

use sea_orm::DatabaseConnection;

use crate::{
    lists::source_values,
    localization,
    projects::{
        PageSource, PageSourceKind, ProjectRepository, ProjectState, ReplacementRule, RuleMask,
        RuleOperation,
    },
    wikis::{self},
};

use super::projects::{LegacySourceLists, NamespaceOption, PageSourceView, UpdateProjectForm};

pub(super) fn split_lines(value: &str) -> Vec<String> {
    value
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_owned)
        .collect()
}

pub(super) fn page_sources_from_form(form: &UpdateProjectForm) -> Result<Vec<PageSource>, String> {
    if form.source_kind.len() != form.source_value.len() {
        return Err("The submitted page sources are incomplete. Reload and try again.".to_owned());
    }
    if form.source_kind.len() > 100 {
        return Err("A project can contain at most 100 page sources.".to_owned());
    }

    form.source_kind
        .iter()
        .zip(&form.source_value)
        .filter_map(|(kind, value)| {
            let values = split_lines(value);
            if values.is_empty() {
                return None;
            }
            Some(parse_page_source(kind, values))
        })
        .collect()
}

fn parse_page_source(kind: &str, mut values: Vec<String>) -> Result<PageSource, String> {
    let kind = match kind {
        "manual" => PageSourceKind::Manual,
        "category" => PageSourceKind::Category,
        "backlinks" => PageSourceKind::Backlinks,
        "transclusions" => PageSourceKind::Transclusions,
        "search" => PageSourceKind::Search,
        "watchlist" => PageSourceKind::Watchlist,
        "contributions" => PageSourceKind::Contributions,
        "links_on_page" => PageSourceKind::LinksOnPage,
        "categories_on_page" => PageSourceKind::CategoriesOnPage,
        "redirects_to_page" => PageSourceKind::RedirectsToPage,
        _ => return Err("A page source has an unsupported type.".to_owned()),
    };
    if kind == PageSourceKind::Watchlist {
        values = vec!["My watchlist".to_owned()];
    }
    if matches!(
        kind,
        PageSourceKind::Category | PageSourceKind::Transclusions
    ) {
        let prefix = if kind == PageSourceKind::Category {
            "Category:"
        } else {
            "Template:"
        };
        for value in &mut values {
            if !value.contains(':') {
                *value = format!("{prefix}{value}");
            }
        }
    }
    source_values(&values, &source_label(kind))?;
    Ok(PageSource { kind, values })
}

pub(super) fn legacy_sources(sources: &[PageSource]) -> LegacySourceLists {
    let (mut pages, mut categories, mut backlinks, mut transclusions, mut searches) =
        (Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new());
    for source in sources {
        match source.kind {
            PageSourceKind::Manual => pages.extend(source.values.clone()),
            PageSourceKind::Category => categories.extend(source.values.clone()),
            PageSourceKind::Backlinks => backlinks.extend(source.values.clone()),
            PageSourceKind::Transclusions => transclusions.extend(source.values.clone()),
            PageSourceKind::Search => searches.extend(source.values.clone()),
            PageSourceKind::Watchlist
            | PageSourceKind::Contributions
            | PageSourceKind::LinksOnPage
            | PageSourceKind::CategoriesOnPage
            | PageSourceKind::RedirectsToPage => {}
        }
    }
    LegacySourceLists {
        pages,
        categories,
        backlinks,
        transclusions,
        searches,
    }
}

pub(super) fn page_source_views(sources: &[PageSource]) -> Vec<PageSourceView> {
    sources
        .iter()
        .map(|source| {
            let first = source
                .values
                .first()
                .map(String::as_str)
                .unwrap_or_default();
            let summary = if source.kind == PageSourceKind::Watchlist {
                localization::required("findsmith-project-source-watchlist")
            } else if source.values.len() > 1 {
                localization::required_with_args(
                    "findsmith-api-source-more",
                    &[first, &(source.values.len() - 1).to_string()],
                )
            } else {
                first.to_owned()
            };
            PageSourceView {
                kind: source.kind.form_value(),
                label: source_label(source.kind),
                value: source.values.join("\n"),
                summary,
            }
        })
        .collect()
}

fn source_label(kind: PageSourceKind) -> String {
    localization::required(&format!(
        "findsmith-project-source-{}",
        kind.form_value().replace('_', "-"),
    ))
}

pub(super) fn namespace_options(
    available: &[wikis::Namespace],
    selected: &[i64],
) -> Vec<NamespaceOption> {
    available
        .iter()
        .map(|namespace| NamespaceOption {
            id: namespace.id,
            label: namespace.name.clone(),
            selected: selected.contains(&namespace.id),
        })
        .collect()
}

pub(super) fn legacy_namespaces() -> Vec<wikis::Namespace> {
    [
        (0, "Main"),
        (1, "Talk"),
        (2, "User"),
        (3, "User talk"),
        (4, "Project"),
        (5, "Project talk"),
        (6, "File"),
        (7, "File talk"),
        (8, "MediaWiki"),
        (9, "MediaWiki talk"),
        (10, "Template"),
        (11, "Template talk"),
        (12, "Help"),
        (13, "Help talk"),
        (14, "Category"),
        (15, "Category talk"),
        (100, "Portal"),
        (101, "Portal talk"),
        (828, "Module"),
        (829, "Module talk"),
    ]
    .into_iter()
    .map(|(id, name)| wikis::Namespace {
        id,
        name: name.into(),
    })
    .collect()
}

pub(super) async fn namespace_options_for_form(
    database: &DatabaseConnection,
    wiki_id: &str,
    selected: &[i64],
) -> Vec<NamespaceOption> {
    let available = match wikis::by_id(database, wiki_id).await {
        Ok(Some(wiki)) => wikis::namespaces(&wiki).unwrap_or_else(|_| legacy_namespaces()),
        _ => legacy_namespaces(),
    };
    namespace_options(&available, selected)
}

pub(super) async fn category_prefix_for_wiki(
    database: &DatabaseConnection,
    wiki_url: &str,
) -> String {
    if let Ok(Some(wiki)) = wikis::by_url(database, wiki_url).await
        && let Ok(namespaces) = wikis::namespaces(&wiki)
        && let Some(category) = namespaces.into_iter().find(|namespace| namespace.id == 14)
        && !category.name.trim().is_empty()
    {
        return category.name;
    }
    "Category".to_owned()
}

pub(super) async fn resolve_project_wiki_url(
    database: &DatabaseConnection,
    project_id: &str,
    owner: &str,
    wiki_id: &str,
) -> Result<Option<String>, sea_orm::DbErr> {
    if let Some(wiki) = wikis::by_id(database, wiki_id).await? {
        return Ok(Some(wiki.api_url));
    }
    if let Ok(Some(project)) = ProjectRepository::new(database)
        .get(owner, project_id)
        .await
        && wiki_id == project.wiki_url
    {
        return Ok(Some(project.wiki_url));
    }
    Ok(None)
}

pub(super) fn validate_project_sources(state: &ProjectState) -> Result<(), String> {
    for source in state.ordered_sources() {
        source_values(&source.values, &source_label(source.kind))?;
    }
    Ok(())
}

pub(super) fn parse_namespaces(values: &[String]) -> Result<Vec<i64>, String> {
    values
        .iter()
        .map(String::as_str)
        .map(|item| {
            item.parse::<i64>()
                .map_err(|_| format!("“{item}” is not a namespace ID."))
        })
        .collect()
}

pub(super) fn replacement_rules(form: &UpdateProjectForm) -> Result<Vec<ReplacementRule>, String> {
    let count = form.rule_find.len();
    if form.rule_replace.len() != count
        || form.rule_type.len() != count
        || form.rule_case.len() != count
        || form.rule_scope.len() != count
        || (!form.rule_operation.is_empty() && form.rule_operation.len() != count)
        || (!form.rule_parameter.is_empty() && form.rule_parameter.len() != count)
        || (!form.rule_name.is_empty() && form.rule_name.len() != count)
    {
        return Err(
            "The submitted rule fields are incomplete. Reload the page and try again.".to_owned(),
        );
    }

    let enabled = rule_indexes(&form.rule_enabled, count)?;
    let multiline = rule_indexes(&form.rule_multiline, count)?;
    let dot_newline = rule_indexes(&form.rule_dot_newline, count)?;
    let mask_comments = rule_indexes(&form.rule_mask_comments, count)?;
    let mask_tags = rule_indexes(&form.rule_mask_tags, count)?;
    let mask_references = rule_indexes(&form.rule_mask_references, count)?;
    let mask_links = rule_indexes(&form.rule_mask_links, count)?;
    let mask_headings = rule_indexes(&form.rule_mask_headings, count)?;
    let mask_templates = rule_indexes(&form.rule_mask_templates, count)?;
    let mask_link_targets = rule_indexes(&form.rule_mask_link_targets, count)?;
    let mut rules = Vec::with_capacity(count);
    for index in 0..count {
        let name = form.rule_name.get(index).map(String::as_str).unwrap_or("");
        if name.chars().count() > 255 {
            return Err(localization::required_with_args(
                "findsmith-api-rule-name-too-long",
                &[&(index + 1).to_string()],
            ));
        }
        if form.rule_find[index].is_empty() && form.rule_replace[index].is_empty() {
            continue;
        }
        let operation = match form
            .rule_operation
            .get(index)
            .map(String::as_str)
            .unwrap_or("replace")
        {
            "replace" => RuleOperation::Replace,
            "append" => RuleOperation::Append,
            "prepend" => RuleOperation::Prepend,
            "category_add" => RuleOperation::CategoryAdd,
            "category_remove" => RuleOperation::CategoryRemove,
            "category_replace" => RuleOperation::CategoryReplace,
            "template_rename" => RuleOperation::TemplateRename,
            "template_parameter_set" => RuleOperation::TemplateParameterSet,
            "template_parameter_remove" => RuleOperation::TemplateParameterRemove,
            _ => {
                return Err(format!("Rule {} has an invalid operation.", index + 1));
            }
        };
        let regex = match form.rule_type[index].as_str() {
            "plain" => false,
            "regex" => true,
            _ => {
                return Err(format!("Rule {} has an invalid type.", index + 1));
            }
        };
        let case_sensitive = match form.rule_case[index].as_str() {
            "sensitive" => true,
            "insensitive" => false,
            _ => {
                return Err(format!("Rule {} has an invalid case option.", index + 1));
            }
        };
        let replace_all = match form.rule_scope[index].as_str() {
            "all" => true,
            "first" => false,
            _ => {
                return Err(format!(
                    "Rule {} has an invalid replacement scope.",
                    index + 1
                ));
            }
        };
        rules.push(ReplacementRule {
            name: name.to_owned(),
            enabled: enabled.contains(&index),
            operation,
            parameter: form.rule_parameter.get(index).cloned().unwrap_or_default(),
            mask: RuleMask {
                comments: mask_comments.contains(&index),
                tags: mask_tags.contains(&index),
                references: mask_references.contains(&index),
                links: mask_links.contains(&index),
                headings: mask_headings.contains(&index),
                templates: mask_templates.contains(&index),
                link_targets: mask_link_targets.contains(&index),
            },
            find: form.rule_find[index].clone(),
            replace: form.rule_replace[index].clone(),
            regex,
            case_sensitive,
            replace_all,
            multiline: multiline.contains(&index),
            dot_matches_new_line: dot_newline.contains(&index),
        });
    }
    Ok(rules)
}

fn rule_indexes(values: &[String], count: usize) -> Result<HashSet<usize>, String> {
    values
        .iter()
        .map(|value| {
            value
                .parse::<usize>()
                .map_err(|_| "A submitted rule index is invalid.".to_owned())
                .and_then(|index| {
                    if index < count {
                        Ok(index)
                    } else {
                        Err("A submitted rule index is out of range.".to_owned())
                    }
                })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skip::SkipOptions;

    #[test]
    fn source_parser_adds_missing_mediawiki_prefixes() {
        let category = parse_page_source("category", vec!["Scientists".to_owned()]).unwrap();
        let template =
            parse_page_source("transclusions", vec!["Infobox person".to_owned()]).unwrap();
        let backlinks = parse_page_source("backlinks", vec!["Example".to_owned()]).unwrap();

        assert_eq!(category.values, ["Category:Scientists"]);
        assert_eq!(template.values, ["Template:Infobox person"]);
        assert_eq!(template.kind.form_value(), "transclusions");
        assert_eq!(backlinks.kind.form_value(), "backlinks");
    }

    #[test]
    fn source_parser_keeps_localized_prefixes() {
        let category =
            parse_page_source("category", vec!["Категорія:Науковиці".to_owned()]).unwrap();
        assert_eq!(category.values, ["Категорія:Науковиці"]);
    }

    #[test]
    fn submitted_rule_keeps_empty_replacement() {
        let form = UpdateProjectForm {
            name: "Example".to_owned(),
            wiki_id: "examplewiki".to_owned(),
            source_kind: Vec::new(),
            source_value: Vec::new(),
            namespaces: Vec::new(),
            queue_limit: 100,
            rule_name: vec!["Remove obsolete".to_owned()],
            rule_operation: vec!["replace".to_owned()],
            rule_parameter: Vec::new(),
            rule_mask_comments: Vec::new(),
            rule_mask_tags: Vec::new(),
            rule_mask_references: Vec::new(),
            rule_mask_links: Vec::new(),
            rule_mask_headings: Vec::new(),
            rule_mask_templates: Vec::new(),
            rule_mask_link_targets: Vec::new(),
            rule_find: vec!["obsolete".to_owned()],
            rule_replace: vec![String::new()],
            rule_enabled: vec!["0".to_owned()],
            rule_type: vec!["plain".to_owned()],
            rule_case: vec!["sensitive".to_owned()],
            rule_scope: vec!["all".to_owned()],
            rule_multiline: Vec::new(),
            rule_dot_newline: Vec::new(),
            skip_no_change: true,
            skip_in_use: true,
            skip_redirects: false,
            skip_protected: false,
            skip_outside_namespaces: true,
            skip_contains: String::new(),
            skip_not_contains: String::new(),
            skip_regex: false,
            skip_case_sensitive: true,
            skip_min_replacements: 0,
            skip_max_replacements: 0,
            typos_enabled: false,
            skip_no_typo: false,
            page_title: String::new(),
            state_version: 1,
            csrf_token: "test".to_owned(),
        };
        assert_eq!(form.skip_options(), SkipOptions::default());

        let rules = replacement_rules(&form).unwrap();
        assert_eq!(rules.len(), 1);
        assert!(rules[0].enabled);
        assert_eq!(rules[0].find, "obsolete");
        assert_eq!(rules[0].name, "Remove obsolete");
        assert_eq!(rules[0].replace, "");
    }
}
