//! GET /api/v1/container-images — curated list of agent images.
//!
//! The dashboard's Create Instance dialog (#178) populates its image dropdown
//! from this endpoint. The list is the set of agent images shipped by the
//! build pipeline (#175) and is currently static — it changes only when the
//! `images/container/Dockerfile.*` set changes, not at runtime.

use axum::Json;
use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct ContainerImage {
    /// Full image reference, e.g. `agentic/claude:latest`. Sent verbatim to
    /// `POST /api/v1/containers` as the `image` field.
    #[serde(rename = "ref")]
    pub image_ref: &'static str,
    /// Short label for UI display.
    pub label: &'static str,
    /// One-line description of what's preinstalled.
    pub description: &'static str,
    /// Whether this image should be the default selection in the picker.
    /// Exactly one entry should be true.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub default: bool,
}

#[derive(Debug, Serialize)]
pub struct ContainerImagesResponse {
    pub images: &'static [ContainerImage],
}

/// Source-of-truth list. Mirrors the Dockerfiles under `images/container/`.
/// Update when a new agent image lands in the build pipeline.
const IMAGES: &[ContainerImage] = &[
    ContainerImage {
        image_ref: "agentic/claude:latest",
        label: "Claude",
        description: "Anthropic Claude Code agent",
        default: true,
    },
    ContainerImage {
        image_ref: "agentic/codex:latest",
        label: "Codex",
        description: "OpenAI Codex agent",
        default: false,
    },
    ContainerImage {
        image_ref: "agentic/opencode:latest",
        label: "OpenCode",
        description: "OpenCode agent",
        default: false,
    },
    ContainerImage {
        image_ref: "agentic/automation-control:latest",
        label: "Automation Control",
        description: "Orchestrator-ready control image with Codex, Aider, dev tools, and credential-free probes",
        default: false,
    },
];

/// `GET /api/v1/container-images`
pub async fn list_container_images() -> Json<ContainerImagesResponse> {
    Json(ContainerImagesResponse { images: IMAGES })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exactly_one_default() {
        let count = IMAGES.iter().filter(|i| i.default).count();
        assert_eq!(count, 1, "exactly one image should be marked default");
    }

    #[test]
    fn all_refs_are_non_empty() {
        for img in IMAGES {
            assert!(!img.image_ref.is_empty());
            assert!(!img.label.is_empty());
            assert!(
                img.image_ref.contains(':'),
                "image ref must include tag: {}",
                img.image_ref
            );
        }
    }
}
