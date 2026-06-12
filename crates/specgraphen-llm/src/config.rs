use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmConfig {
    pub provider: ProviderType,
    pub api_key: String,
    pub model: String,
    pub base_url: Option<String>,
    pub max_tokens: u32,
    pub temperature: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ProviderType {
    Claude,
    OpenAi,
}

impl Default for LlmConfig {
    fn default() -> Self {
        Self {
            provider: ProviderType::Claude,
            api_key: String::new(),
            model: "claude-sonnet-4-20250514".to_string(),
            base_url: None,
            max_tokens: 4096,
            temperature: 0.0,
        }
    }
}

impl LlmConfig {
    pub fn create_provider(&self) -> anyhow::Result<Box<dyn crate::LlmProvider>> {
        match self.provider {
            ProviderType::Claude => Ok(Box::new(crate::claude::ClaudeProvider::new(self)?)),
            ProviderType::OpenAi => Ok(Box::new(crate::openai::OpenAiProvider::new(self)?)),
        }
    }
}
