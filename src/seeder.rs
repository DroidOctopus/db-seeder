// src/seeder.rs
use crate::config::SeedingTask;
use crate::db::{DbClient, DbSchema};
use crate::entity_generator::{DataPools, EntityGenerator};
use crate::error::{AppError, AppResult};
use crate::gemini_analyzer::{ArchitecturalPlan, GeminiAnalyzer};
use console::style;
use indicatif::ProgressBar;
use petgraph::algo::toposort;
use petgraph::graphmap::DiGraphMap;
use serde_json::{json, Value};
use sqlx::Row;
use std::collections::{HashMap, HashSet};

pub struct Seeder {
    db_client: DbClient,
    schema: DbSchema,
}

impl Seeder {
    pub async fn new(db_client: DbClient) -> AppResult<Self> {
        println!("🔎 Читаю схему бази даних...");
        let schema = db_client.fetch_schema().await?;
        Ok(Self { db_client, schema })
    }

    // Метод для публічного доступу (для інтерактивного режиму)
    pub fn schema(&self) -> &DbSchema {
        &self.schema
    }

    // Ця функція потрібна для інтерактивного режиму, щоб показати всі залежності
    pub fn build_full_dependency_graph(&self) -> DiGraphMap<&str, ()> {
        let mut graph = DiGraphMap::new();
        for table_name in self.schema.tables.keys() {
            graph.add_node(table_name.as_str());
        }
        for fk in &self.schema.foreign_keys {
            if self.schema.tables.contains_key(&fk.from_table) && self.schema.tables.contains_key(&fk.to_table) {
                graph.add_edge(fk.to_table.as_str(), fk.from_table.as_str(), ());
            }
        }
        graph
    }

    // Ця функція використовується всередині `run`
    fn build_plan_dependency_graph<'a>(&self, plan: &'a ArchitecturalPlan) -> DiGraphMap<&'a str, ()> {
        let mut graph = DiGraphMap::new();
        
        let entity_map: HashMap<&str, &'a str> = plan.entity_templates.iter()
            .map(|t| (t.target_table.as_str(), t.entity_name.as_str()))
            .collect();

        for &entity_name in entity_map.values() {
            graph.add_node(entity_name);
        }

        for fk in &self.schema.foreign_keys {
            if let (Some(&parent_entity), Some(&child_entity)) = (entity_map.get(fk.to_table.as_str()), entity_map.get(fk.from_table.as_str())) {
                graph.add_edge(parent_entity, child_entity, ());
            }
        }
        graph
    }

    pub async fn run(&self, config: &crate::config::AppConfig) -> AppResult<()> {
        let gemini_key = std::env::var("GEMINI_API_KEY")
            .map_err(|_| AppError::Custom("Змінна середовища GEMINI_API_KEY не встановлена".to_string()))?;
        
        let model = config.gemini.as_ref().map_or("gemini-1.5-flash-latest".to_string(), |g| g.model.clone());
        let lang = config.generation.as_ref().map_or("en", |g| &g.language);
        
        let analyzer = GeminiAnalyzer::new(gemini_key, model);

        println!("🧠 Gemini розробляє архітектурний план (мова: {})...", lang);
        
        let plan_tasks = config.plan.as_ref().ok_or_else(|| AppError::Custom("Секція [[plan]] відсутня в конфігурації".to_string()))?;

        let all_table_names: HashSet<&str> = plan_tasks.iter().map(|t| t.table.as_str()).collect();
        let schemas_for_analysis: Vec<_> = self.schema.tables.values().filter(|t| all_table_names.contains(t.name.as_str())).collect();
        if schemas_for_analysis.is_empty() {
            println!("{}", style("Не знайдено таблиць для аналізу в схемі БД. Перевірте `plan` в конфігурації.").yellow());
            return Ok(());
        }
        
        let architectural_plan = analyzer.get_architectural_plan(&schemas_for_analysis, lang).await?;
        println!("✅ План отримано! Тема: {}", style(&architectural_plan.theme).green());

        let mut data_pools = DataPools::new();
        if !architectural_plan.data_pools.is_empty() {
            println!("💧 Заповнюю пули даних за допомогою Gemini...");
            let bar = ProgressBar::new(architectural_plan.data_pools.len() as u64);
            for (pool_name, pool_config) in &architectural_plan.data_pools {
                bar.set_message(format!("Генерую пул '{}'", pool_name));
                let pool_data = analyzer.get_pool_data(&pool_config.gemini_prompt_for_pool).await?;
                let pool_values: Vec<Value> = pool_data.into_iter().map(Value::String).collect();
                data_pools.insert(pool_name.clone(), pool_values);
                bar.inc(1);
            }
            bar.finish_with_message("✅ Пули даних заповнено!");
        }

        let entity_generator = EntityGenerator::new();
        let mut generated_pks: DataPools = HashMap::new();
        
        let graph = self.build_plan_dependency_graph(&architectural_plan);
        let sorted_entities = toposort(&graph, None).map_err(|_| AppError::CyclicDependency)?;

        println!("\n🚀 Порядок заповнення сутностей визначено:");
        for (i, entity_name) in sorted_entities.iter().enumerate() {
            println!("   {}. {}", i + 1, style(entity_name).cyan());
        }

        for entity_name in sorted_entities {
            if let Some(entity_template) = architectural_plan.entity_templates.iter().find(|e| e.entity_name == *entity_name) {
                if let Some(task) = plan_tasks.iter().find(|t| t.table == entity_template.target_table) {
                    println!("\n🌱 Заповнюю таблицю '{}' ({} рядків) сутностями '{}'...", style(&task.table).bold(), task.rows, style(entity_name).cyan());
                    
                    let pks = self.seed_table(task, entity_template, &entity_generator, &data_pools, &generated_pks).await?;
                    if !pks.is_empty() {
                        generated_pks.insert(entity_template.target_table.clone(), pks);
                    }
                }
            }
        }
        
        println!("\n✨ Заповнення бази даних успішно завершено!");
        Ok(())
    }

    async fn seed_table(
        &self,
        task: &SeedingTask,
        template: &crate::gemini_analyzer::EntityTemplate,
        generator: &EntityGenerator,
        pools: &DataPools,
        all_previous_pks: &DataPools,
    ) -> AppResult<Vec<Value>> {
        let bar = ProgressBar::new(task.rows as u64);
        let mut generated_pks_for_this_table = Vec::new();
        
        let table_schema = self.schema.tables.get(&template.target_table)
            .ok_or_else(|| AppError::Custom(format!("Схема для таблиці '{}' не знайдена", template.target_table)))?;

        let pk_col_name = table_schema.primary_key_column.as_deref();

        let mut tx = self.db_client.pool().begin().await?;
        for _ in 0..task.rows {
            let mut available_pks = all_previous_pks.clone();
            available_pks.insert(template.target_table.clone(), generated_pks_for_this_table.clone());
            let entity = generator.generate_entity(&template.fields, pools, &available_pks)?;

            let columns: Vec<String> = entity.keys().cloned().collect();
            let values: Vec<Value> = columns.iter().map(|k| entity.get(k).unwrap().clone()).collect();
            let column_names = columns.iter().map(|s| format!("\"{}\"", s)).collect::<Vec<_>>().join(", ");
            
            let placeholders: String = columns.iter().enumerate().map(|(i, col_name)| {
                let placeholder_index = i + 1;
                let col_schema = table_schema.columns.iter().find(|c| &c.name == col_name);
                if let Some(schema) = col_schema {
                    match schema.data_type.as_str() {
                        "timestamp with time zone" | "timestamp without time zone" => format!("${}::timestamp", placeholder_index),
                        "date" => format!("${}::date", placeholder_index),
                        "uuid" => format!("${}::uuid", placeholder_index),
                        _ => format!("${}", placeholder_index),
                    }
                } else {
                    format!("${}", placeholder_index)
                }
            }).collect::<Vec<_>>().join(", ");
            
            let mut sql = format!("INSERT INTO \"{}\" ({}) VALUES ({})", template.target_table, column_names, placeholders);

            if let Some(pk_name) = pk_col_name {
                sql.push_str(&format!(" RETURNING \"{}\"", pk_name));
            }
            
            let mut query = sqlx::query(&sql);
            for (i, col_name) in columns.iter().enumerate() {
                let val = &values[i];
                let col_schema = table_schema.columns.iter().find(|c| &c.name == col_name);

                if let Some(schema) = col_schema {
                    match schema.data_type.as_str() {
                        "integer" | "bigint" | "smallint" | "int4" => {
                            // Примусово перетворюємо в число
                            let int_val = if let Some(i) = val.as_i64() {
                                i
                            } else if let Some(b) = val.as_bool() {
                                if b { 1 } else { 0 }
                            } else if let Some(s) = val.as_str() {
                                s.parse::<i64>().unwrap_or(0)
                            } else {
                                0 // Дефолтне значення
                            };
                            query = query.bind(int_val);
                        }
                        "boolean" => {
                            // Якщо в БД тип boolean
                            let bool_val = if let Some(b) = val.as_bool() {
                                b
                            } else if let Some(i) = val.as_i64() {
                                i != 0
                            } else if let Some(s) = val.as_str() {
                                s == "true" || s == "1"
                            } else {
                                false
                            };
                            query = query.bind(bool_val);
                        }
                        "character varying" | "text" | "varchar" | "uuid" | "timestamp with time zone" | "timestamp without time zone" | "date" => {
                            // Для цих типів ми покладаємося на кастинг в SQL (::timestamp, ::uuid)
                            // і просто передаємо рядок
                            query = query.bind(val.as_str().unwrap_or("").to_string());
                        }
                        _ => {
                            // Для всіх інших (json, numeric, etc.)
                            query = query.bind(val);
                        }
                    }
                } else {
                    // Якщо схему не знайдено, біндимо як є
                    query = query.bind(val);
                }
            }
            
            if let Some(pk_name) = pk_col_name {
                let row = query.fetch_one(&mut *tx).await?;

                let pk_col_schema = table_schema.columns.iter().find(|c| &c.name == pk_name)
                    .ok_or_else(|| AppError::Custom(format!("Не знайдено схему для PK колонки {}", pk_name)))?;

                let pk_val: Value = match pk_col_schema.data_type.as_str() {
                    "character varying" | "text" | "varchar" | "uuid" => {
                        let val: String = row.get(0);
                        Value::String(val)
                    },
                    "integer" | "smallint" => {
                        let val: i32 = row.get(0);
                        json!(val)
                    },
                    "bigint" => {
                        let val: i64 = row.get(0);
                        json!(val)
                    }
                    _ => return Err(AppError::Custom(format!("Непідтримуваний тип даних для первинного ключа: {}", pk_col_schema.data_type)))
                };
                
                generated_pks_for_this_table.push(pk_val);
            } else {
                query.execute(&mut *tx).await?;
            }
            bar.inc(1);
        }
        tx.commit().await?;

        bar.finish_with_message("Завершено");
        Ok(generated_pks_for_this_table)
    }
}