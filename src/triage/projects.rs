use crate::config::ProjectsConfig;
use crate::github::client::GitHubClient;
use anyhow::{Context, Result};
use serde::Deserialize;
use serde_json::json;
use tracing::info;

pub struct ProjectsService {
    config: ProjectsConfig,
}

#[derive(Debug, Deserialize)]
struct OrgProjectQueryData {
    organization: Option<ProjectContainer>,
    user: Option<ProjectContainer>,
}

#[derive(Debug, Deserialize)]
struct ProjectContainer {
    #[serde(rename = "projectV2")]
    project_v2: Option<ProjectV2Details>,
}

#[derive(Debug, Deserialize)]
struct ProjectV2Details {
    id: String,
    fields: ProjectFieldsConnection,
}

#[derive(Debug, Deserialize)]
struct ProjectFieldsConnection {
    nodes: Vec<ProjectFieldNode>,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum ProjectFieldNode {
    SingleSelect {
        id: String,
        name: String,
        options: Vec<SelectOption>,
    },
    Other {},
}

#[derive(Debug, Deserialize)]
struct SelectOption {
    id: String,
    name: String,
}

#[derive(Debug, Deserialize)]
struct AddItemResponseData {
    #[serde(rename = "addProjectV2ItemById")]
    add_project_v2_item_by_id: Option<AddItemPayload>,
}

#[derive(Debug, Deserialize)]
struct AddItemPayload {
    item: ProjectItemNode,
}

#[derive(Debug, Deserialize)]
struct ProjectItemNode {
    id: String,
}

impl ProjectsService {
    pub fn new(config: ProjectsConfig) -> Self {
        Self { config }
    }

    pub async fn add_item_to_project(
        &self,
        client: &GitHubClient,
        content_node_id: &str, // The GraphQL node ID of the Issue or PR
        target_owner: Option<&str>,
        target_number: Option<i64>,
        target_status: Option<&str>,
    ) -> Result<()> {
        let owner = target_owner
            .or(self.config.project_owner.as_deref())
            .context("Project owner not configured")?;
        let number = target_number
            .or(self.config.project_number)
            .context("Project number not configured")?;

        let (project_id, status_field_id, option_id) = self
            .find_project_and_status(
                client,
                owner,
                number,
                target_status.or(self.config.initial_status.as_deref()),
            )
            .await?;

        // Add item to project
        let add_mutation = r#"
        mutation($projectId: ID!, $contentId: ID!) {
            addProjectV2ItemById(input: { projectId: $projectId, contentId: $contentId }) {
                item {
                    id
                }
            }
        }
        "#;

        let add_vars = json!({
            "projectId": project_id,
            "contentId": content_node_id,
        });

        let add_resp: AddItemResponseData = client
            .graphql(add_mutation, add_vars)
            .await
            .context("Failed to execute addProjectV2ItemById mutation")?;

        let item_id = match add_resp.add_project_v2_item_by_id {
            Some(payload) => payload.item.id,
            None => anyhow::bail!("No item ID returned after adding to project"),
        };

        info!(
            "Added item {} to project {} (item ID: {})",
            content_node_id, project_id, item_id
        );

        // Update status field if option_id is available
        if let (Some(field_id), Some(opt_id)) = (status_field_id, option_id) {
            let update_status_mutation = r#"
            mutation($projectId: ID!, $itemId: ID!, $fieldId: ID!, $singleSelectOptionId: String!) {
                updateProjectV2ItemFieldValue(input: {
                    projectId: $projectId
                    itemId: $itemId
                    fieldId: $fieldId
                    value: { singleSelectOptionId: $singleSelectOptionId }
                }) {
                    projectV2Item {
                        id
                    }
                }
            }
            "#;

            let update_vars = json!({
                "projectId": project_id,
                "itemId": item_id,
                "fieldId": field_id,
                "singleSelectOptionId": opt_id,
            });

            let _: serde_json::Value = client
                .graphql(update_status_mutation, update_vars)
                .await
                .context("Failed to update project item status field")?;

            info!(
                "Updated status for item {} in project {}",
                item_id, project_id
            );
        }

        Ok(())
    }

    async fn find_project_and_status(
        &self,
        client: &GitHubClient,
        owner: &str,
        number: i64,
        target_status_name: Option<&str>,
    ) -> Result<(String, Option<String>, Option<String>)> {
        let query = r#"
        query($owner: String!, $number: Int!) {
            organization(login: $owner) {
                projectV2(number: $number) {
                    id
                    fields(first: 30) {
                        nodes {
                            ... on ProjectV2SingleSelectField {
                                id
                                name
                                options {
                                    id
                                    name
                                }
                            }
                        }
                    }
                }
            }
            user(login: $owner) {
                projectV2(number: $number) {
                    id
                    fields(first: 30) {
                        nodes {
                            ... on ProjectV2SingleSelectField {
                                id
                                name
                                options {
                                    id
                                    name
                                }
                            }
                        }
                    }
                }
            }
        }
        "#;

        let vars = json!({
            "owner": owner,
            "number": number,
        });

        let data: OrgProjectQueryData = client.graphql(query, vars).await?;
        let proj = data
            .organization
            .and_then(|o| o.project_v2)
            .or_else(|| data.user.and_then(|u| u.project_v2))
            .context("Project V2 not found for specified owner and number")?;

        let mut status_field_id = None;
        let mut target_option_id = None;

        if let Some(target_status) = target_status_name {
            for field in proj.fields.nodes {
                if let ProjectFieldNode::SingleSelect { id, name, options } = field {
                    if name.eq_ignore_ascii_case("status") {
                        status_field_id = Some(id);
                        for opt in options {
                            if opt.name.eq_ignore_ascii_case(target_status) {
                                target_option_id = Some(opt.id);
                                break;
                            }
                        }
                        break;
                    }
                }
            }
        }

        Ok((proj.id, status_field_id, target_option_id))
    }
}
