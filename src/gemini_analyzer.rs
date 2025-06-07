// src/gemini_analyzer.rs
use crate::db::TableSchema;
use crate::error::{AppError, AppResult};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::HashMap;
use serde_json::Value;
use tokio::time::{sleep, Duration};

// --- Структури для відповіді від Gemini ---
#[derive(Deserialize, Debug)]
struct GeminiResponse { candidates: Vec<Candidate> }
#[derive(Deserialize, Debug)]
struct Candidate { content: Content }
#[derive(Deserialize, Debug)]
struct Content { parts: Vec<Part> }
#[derive(Deserialize, Debug)]
struct Part { text: String }

// --- Нові структури "Архітектурного Плану" ---

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct DataPoolConfig {
    pub description: String,
    pub uniqueness_ratio: f32,
    pub gemini_prompt_for_pool: String,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct FieldTemplate {
    pub column_name: String,
    pub generator: String, // "pk_hash", "from_pool", "template", "datetime_range", "fk", "words", "sentence"
    #[serde(default)]
    pub params: HashMap<String, serde_json::Value>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct EntityTemplate {
    pub entity_name: String,
    pub target_table: String,
    pub fields: Vec<FieldTemplate>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct ArchitecturalPlan {
    pub theme: String,
    pub data_pools: HashMap<String, DataPoolConfig>,
    pub entity_templates: Vec<EntityTemplate>,
}

pub struct GeminiAnalyzer {
    http_client: Client,
    api_key: String,
    model: String,
}

impl GeminiAnalyzer {
    pub fn new(api_key: String, model: String) -> Self {
        Self { http_client: Client::new(), api_key, model }
    }

    /// Запитує у Gemini архітектурний план
    pub async fn get_architectural_plan(&self, schemas: &[&TableSchema], lang: &str) -> AppResult<ArchitecturalPlan> {
        let prompt = self.build_plan_prompt(schemas, lang);
        let json_text = self.query_gemini(&prompt).await?;
        let plan: ArchitecturalPlan = serde_json::from_str(&json_text)
            .map_err(|e| AppError::Custom(format!("Помилка парсингу плану від Gemini: {}. Відповідь: {}", e, json_text)))?;
        Ok(plan)
    }

    /// Запитує у Gemini дані для заповнення конкретного пулу
    pub async fn get_pool_data(&self, prompt: &str) -> AppResult<Vec<String>> {
        // ВИПРАВЛЕНО: Додаємо системну обгортку до промпту
        let final_prompt = format!(
            "Ти - генератор даних. Твоя єдина задача - виконати наступну інструкцію і повернути ЛИШЕ валідний JSON без жодного додаткового тексту, коментарів чи пояснень.\n\nІнструкція: {}",
            prompt
        );

        const MAX_RETRIES: u32 = 3;
        for attempt in 0..MAX_RETRIES {
            match self.query_gemini(&final_prompt).await {
                Ok(json_text) => {
                    // Якщо отримали відповідь, намагаємося її розпарсити
                    match self.parse_pool_response(&json_text) {
                        Ok(data) => return Ok(data), // Успіх, виходимо
                        Err(e) => {
                            // Помилка парсингу, логуємо і спробуємо ще раз
                            eprintln!("⚠️ Спроба {}: Помилка парсингу відповіді для пулу. Помилка: {}. Спробую ще раз...", attempt + 1, e);
                        }
                    }
                }
                Err(e) => {
                     // Помилка мережі або API, логуємо і спробуємо ще раз
                    eprintln!("⚠️ Спроба {}: Помилка запиту до Gemini. Помилка: {}. Спробую ще раз...", attempt + 1, e);
                }
            }
            // Чекаємо перед наступною спробою
            sleep(Duration::from_secs(2)).await;
        }

        // Якщо всі спроби провалилися
        Err(AppError::Custom(format!("Не вдалося отримати валідні дані для пулу після {} спроб. Промпт: '{}'", MAX_RETRIES, prompt)))
    }

    fn parse_pool_response(&self, json_text: &str) -> AppResult<Vec<String>> {
        let parsed_value: Value = serde_json::from_str(json_text)
            .map_err(|e| AppError::Custom(format!("Відповідь не є валідним JSON: {}", e)))?;

        let array_to_process = if let Some(obj) = parsed_value.as_object() {
            obj.values().find_map(|v| v.as_array()).map(|a| a.to_vec())
        } else {
            parsed_value.as_array().map(|a| a.to_vec())
        };

        if let Some(array) = array_to_process {
            let mut results = Vec::new();
            for item in array {
                // ВИПРАВЛЕНО: Універсальна обробка елементів пулу
                let value_as_string = if let Some(s) = item.as_str() {
                    Some(s.to_string())
                } else if item.is_number() || item.is_boolean() {
                    Some(item.to_string()) // Перетворюємо число або bool в рядок
                } else if let Some(obj) = item.as_object() {
                    // Для об'єктів, як і раніше, беремо перше значення
                    obj.values()
                       .next()
                       .and_then(|v| v.as_str().map(|s| s.to_string()).or_else(|| Some(v.to_string())))
                } else {
                    None
                };

                if let Some(s) = value_as_string {
                    results.push(s);
                }
            }
            if results.is_empty() {
                return Err(AppError::Custom("Масив даних від Gemini порожній або має непідтримуваний формат елементів.".to_string()));
            }
            Ok(results)
        } else {
            Err(AppError::Custom("Відповідь не є JSON-масивом або об'єктом, що містить масив.".to_string()))
        }
    }

    async fn query_gemini(&self, prompt: &str) -> AppResult<String> {
        if self.api_key.is_empty() { return Err(AppError::Custom("API ключ для Gemini не встановлено".to_string())); }
        
        let url = format!("https://generativelanguage.googleapis.com/v1beta/models/{}:generateContent?key={}", self.model, self.api_key);
        let body = json!({
            "contents": [{ "parts": [{ "text": prompt }] }],
            "generationConfig": { "response_mime_type": "application/json" }
        });

        let response = self.http_client.post(&url).json(&body).send().await?;
        if !response.status().is_success() {
            return Err(AppError::Custom(format!("Помилка від Gemini API: {}", response.text().await?)));
        }

        let gemini_response = response.json::<GeminiResponse>().await?;
        gemini_response
            .candidates.into_iter().next()
            .and_then(|c| c.content.parts.into_iter().next())
            .map(|p| p.text.trim().to_string())
            .ok_or_else(|| AppError::Custom("Gemini API не повернув JSON-текст".to_string()))
    }

    fn build_plan_prompt(&self, schemas: &[&TableSchema], lang: &str) -> String {
        let mut schemas_str = String::new();
        for schema in schemas {
            schemas_str.push_str(&format!("\n--- Table: {} ---\n", schema.name));
            for col in &schema.columns {
                schemas_str.push_str(&format!("- {} (type: {}, nullable: {}, default: {})\n", col.name, col.data_type, col.is_nullable, col.column_default.as_deref().unwrap_or("none")));
            }
        }

        let lang_instruction = if lang == "uk" {
            "Provide all descriptions and data generation prompts in Ukrainian."
        } else {
            "Provide all descriptions and data generation prompts in English."
        };

        // ВИПРАВЛЕНО: Новий, максимально суворий промпт
        format!(r#"
You are a meticulous data architect. Your task is to analyze table schemas and create a detailed JSON plan for data generation.
Your response MUST be ONLY a valid JSON object. Do not add any explanations.
{lang_instruction}

Follow the JSON structure from the example below EXACTLY.
Most importantly, for the "generator" field, you MUST use ONLY one of the values from the "Allowed Generators" list. DO NOT invent new generator names.

### Allowed Generators List ###
- `pk_hash`: For string-based primary keys. (params: {{"length": number}})
- `from_pool`: To get a random value from a data pool. (params: {{"pool_name": "string"}})
- `template`: To combine fields into a new string. (params: {{"format": "string with {{field_name}} placeholders"}})
- `fk`: For foreign keys. (params: {{"references": "table_name"}})
- `words`: For short text (2-5 words). (params: {{"min": number, "max": number}})
- `sentence`: For longer text (1-3 sentences). (params: {{"min": number, "max": number}})
- `number_range`: For all numeric types (integer, decimal). (params: {{"min": number, "max": number}})
- `boolean`: For boolean values. (params: {{"true_chance": float_between_0_and_1}})
- `datetime_range`: For all date and time types (timestamp, date). (params: {{"start": "YYYY-MM-DD", "end": "YYYY-MM-DD"}})

### EXAMPLE OF THE REQUIRED JSON STRUCTURE ###
{{
  "theme": "Users and their blog posts for a tech blog",
  "data_pools": {{
    "first_names": {{
      "description": "A pool of common first names.",
      "uniqueness_ratio": 0.1,
      "gemini_prompt_for_pool": "Provide a JSON array of 100 common first names."
    }}
  }},
  "entity_templates": [
    {{
      "entity_name": "User",
      "target_table": "users",
      "fields": [
        {{ "column_name": "id", "generator": "pk_hash", "params": {{ "length": 12 }} }},
        {{ "column_name": "name", "generator": "from_pool", "params": {{ "pool_name": "first_names" }} }},
        {{ "column_name": "created_at", "generator": "datetime_range", "params": {{ "start": "2022-01-01", "end": "2023-01-01" }} }}
      ]
    }}
  ]
}}
### END OF EXAMPLE ###

Now, analyze the following schemas and generate the plan. Remember to use ONLY the allowed generator names.

### SCHEMAS TO ANALYZE ###
{schemas_str}
"#,
            lang_instruction = lang_instruction,
            schemas_str = schemas_str
        )
    }
}