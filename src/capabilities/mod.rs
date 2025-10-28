mod conversation;
mod image;
mod music;
mod video;

pub use conversation::ConversationAgent;
pub use image::ImageGenerator;
pub use music::MusicGenerator;
pub use video::VideoGenerator;

use serde_json::{Map, Value};

#[derive(Debug, Clone)]
pub struct BinaryArtifact {
    pub data: Vec<u8>,
    pub media_type: String,
    pub file_extension: String,
    pub summary: String,
    pub metadata: Map<String, Value>,
}

impl BinaryArtifact {
    pub fn new(
        data: Vec<u8>,
        media_type: impl Into<String>,
        file_extension: impl Into<String>,
        summary: impl Into<String>,
    ) -> Self {
        Self {
            data,
            media_type: media_type.into(),
            file_extension: file_extension.into(),
            summary: summary.into(),
            metadata: Map::new(),
        }
    }

    pub fn with_metadata(
        data: Vec<u8>,
        media_type: impl Into<String>,
        file_extension: impl Into<String>,
        summary: impl Into<String>,
        metadata: Map<String, Value>,
    ) -> Self {
        Self {
            data,
            media_type: media_type.into(),
            file_extension: file_extension.into(),
            summary: summary.into(),
            metadata,
        }
    }
}
