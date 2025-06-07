// src/config.rs
use serde::Deserialize;
use std::collections::HashMap;

#[derive(Debug, Deserialize, Clone)]
pub struct ColumnOverride {
    pub generator: String,
    pub prompt: Option<String>,
    #[serde(rename = "type")]
    pub kind: Option<String>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct SeedingTask {
    pub table: String,
    pub rows: u32,
    // Ці поля є залишками старої системи, але ми їх залишимо,
    // щоб не ламати парсинг старих конфігів. Вони ігноруються в новій логіці.
    pub columns: Option<Vec<String>>,
    #[serde(default)]
    pub column_overrides: HashMap<String, ColumnOverride>,
    #[serde(default)]
    pub smart_mode: bool,
}

#[derive(Debug, Deserialize)]
pub struct DatabaseConfig {
    pub url: String,
}

#[derive(Debug, Deserialize)]
pub struct GeminiConfig {
    pub model: String,
}

// ВИПРАВЛЕНО: Нова секція для налаштувань генерації
#[derive(Debug, Deserialize)]
pub struct GenerationConfig {
    pub language: String,
}

// ВИПРАВЛЕНО: Єдина, правильна структура AppConfig
#[derive(Debug, Deserialize)]
pub struct AppConfig {
    pub database: DatabaseConfig,
    pub gemini: Option<GeminiConfig>,
    pub generation: Option<GenerationConfig>,

    #[serde(rename = "seeding_plan")]
    pub plan: Option<Vec<SeedingTask>>,
    pub default_rows: Option<u32>,
}

impl AppConfig {
    pub fn from_file(path: &str) -> crate::error::AppResult<Self> {
        let builder = config::Config::builder()
            .add_source(config::File::with_name(path).required(true))
            .add_source(config::Environment::with_prefix("APP"));
            
        let config = builder.build()?.try_deserialize()?;
        Ok(config)
    }
}