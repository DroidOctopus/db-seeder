// src/entity_generator.rs
use crate::error::{AppError, AppResult};
use crate::gemini_analyzer::FieldTemplate;
use fake::{faker, Fake};
use rand::distributions::Alphanumeric;
use rand::Rng;
use serde_json::{json, Value};
use std::collections::HashMap;
use chrono::{DateTime, NaiveDateTime, Utc};

pub type DataPools = HashMap<String, Vec<Value>>;
pub type GeneratedEntity = HashMap<String, Value>;

pub struct EntityGenerator {}

impl EntityGenerator {
    pub fn new() -> Self {
        Self {}
    }

    pub fn generate_entity(
        &self,
        fields: &[FieldTemplate],
        pools: &DataPools,
        foreign_keys: &DataPools,
    ) -> AppResult<GeneratedEntity> {
        let mut entity = GeneratedEntity::new();
        let mut rng = rand::thread_rng();

        for field in fields {
            let value = match field.generator.as_str() {
                "pk_hash" => {
                    let length = field.params.get("length").and_then(|v| v.as_u64()).unwrap_or(20) as usize;
                    let hash: String = (&mut rng).sample_iter(&Alphanumeric).take(length).map(char::from).collect();
                    json!(hash)
                }
                "from_pool" => {
                    let pool_name = field.params.get("pool_name").and_then(|v| v.as_str()).ok_or_else(|| AppError::Custom("`pool_name` не вказано для генератора `from_pool`".to_string()))?;
                    let pool = pools.get(pool_name).ok_or_else(|| AppError::Custom(format!("Пул даних '{}' не знайдено", pool_name)))?;
                    pool[rng.gen_range(0..pool.len())].clone()
                }
                "template" => {
                    let format = field.params.get("format").and_then(|v| v.as_str()).ok_or_else(|| AppError::Custom("`format` не вказано для `template`".to_string()))?;
                    let mut result = format.to_string();
                    for (key, val) in &entity {
                        // Для числових значень теж робимо заміну
                        let val_str = match val {
                            Value::String(s) => s.clone(),
                            Value::Number(n) => n.to_string(),
                            _ => "".to_string(),
                        };
                        if !val_str.is_empty() {
                           result = result.replace(&format!("{{{}}}", key), &val_str);
                        }
                    }
                    if result.contains("{random_digits:") {
                         let num: u32 = rng.gen_range(1000..9999);
                         result = result.replace("{random_digits:4}", &format!("{:04}", num));
                    }
                    json!(result)
                }
                "fk" => {
                    let parent_table = field.params.get("references").and_then(|v| v.as_str()).ok_or_else(|| AppError::Custom("`references` не вказано для `fk`".to_string()))?;
                    let fk_pool = foreign_keys.get(parent_table).ok_or_else(|| AppError::DependencyNotFound(parent_table.to_string()))?;
                    if fk_pool.is_empty() { return Err(AppError::Custom(format!("Батьківський пул ключів для '{}' порожній", parent_table))); }
                    fk_pool[rng.gen_range(0..fk_pool.len())].clone()
                }
                "words" => {
                    let min = field.params.get("min").and_then(|v| v.as_u64()).unwrap_or(2) as usize;
                    let max = field.params.get("max").and_then(|v| v.as_u64()).unwrap_or(5) as usize;
                    let text: Vec<String> = faker::lorem::en::Words(min..max).fake();
                    json!(text.join(" "))
                }
                "number_range" => {
                    let min = field.params.get("min").and_then(|v| v.as_i64()).unwrap_or(0);
                    let max = field.params.get("max").and_then(|v| v.as_i64()).unwrap_or(100);
                    json!(rng.gen_range(min..=max))
                }
                "boolean" => {
                    let true_chance = field.params.get("true_chance").and_then(|v| v.as_f64()).unwrap_or(0.5);
                    json!(rng.gen_bool(true_chance))
                }
                "sentence" => {
                    let min = field.params.get("min").and_then(|v| v.as_u64()).unwrap_or(5) as usize;
                    let max = field.params.get("max").and_then(|v| v.as_u64()).unwrap_or(10) as usize;
                    let text: String = faker::lorem::en::Sentence(min..max).fake();
                    json!(text)
                }
                "datetime_range" => {
                    let start_str = field.params.get("start").and_then(|v| v.as_str()).unwrap_or("2020-01-01");
                    let end_str = field.params.get("end").and_then(|v| v.as_str()).unwrap_or("2024-01-01");

                    let start_dt = NaiveDateTime::parse_from_str(&format!("{} 00:00:00", start_str), "%Y-%m-%d %H:%M:%S")
                        .map(|ndt| ndt.and_utc())
                        .unwrap_or_else(|_| Utc::now());

                    let end_dt = NaiveDateTime::parse_from_str(&format!("{} 23:59:59", end_str), "%Y-%m-%d %H:%M:%S")
                        .map(|ndt| ndt.and_utc())
                        .unwrap_or_else(|_| Utc::now());

                    let start_ts = start_dt.timestamp();
                    let end_ts = end_dt.timestamp();

                    if start_ts >= end_ts {
                        json!(start_dt.to_rfc3339())
                    } else {
                        let random_ts = rng.gen_range(start_ts..=end_ts);
                        let random_dt = DateTime::from_timestamp(random_ts, 0).unwrap_or_else(|| Utc::now());
                        json!(random_dt.to_rfc3339())
                    }
                }
                _ => return Err(AppError::UnknownGenerator(field.generator.clone())),
            };
            entity.insert(field.column_name.clone(), value);
        }
        Ok(entity)
    }
}