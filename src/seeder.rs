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

    pub async fn run(&self, config: &crate::config::AppConfig) -> AppResult<()> {
        let gemini_key = std::env::var("GEMINI_API_KEY")
            .map_err(|_| AppError::Custom("Змінна середовища GEMINI_API_KEY не встановлена".to_string()))?;
        
        let model = config.gemini.as_ref().map_or("gemini-1.5-flash-latest".to_string(), |g| g.model.clone());
        let lang = config.generation.as_ref().map_or("en", |g| &g.language);
        
        let analyzer = GeminiAnalyzer::new(gemini_key, model);

        println!("🧠 Gemini розробляє архітектурний план (мова: {})...", lang);
        
        let plan = config.plan.as_ref().ok_or_else(|| AppError::Custom("Секція [[seeding_plan]] відсутня в конфігурації".to_string()))?;

        let all_table_names: HashSet<&str> = plan.iter().map(|t| t.table.as_str()).collect();
        let schemas_for_analysis: Vec<_> = self.schema.tables.values().filter(|t| all_table_names.contains(t.name.as_str())).collect();
        if schemas_for_analysis.is_empty() {
            println!("{}", style("Не знайдено таблиць для аналізу в схемі БД. Перевірте `seeding_plan` в конфігурації.").yellow());
            return Ok(());
        }
        
        let architectural_plan = analyzer.get_architectural_plan(&schemas_for_analysis, lang).await?;
        println!("✅ План отримано! Тема: {}", style(&architectural_plan.theme).green());

        // --- ФАЗА 2: ПРЕ-ГЕНЕРАЦІЯ ПУЛІВ ---
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

        // --- ФАЗА 3: МАСОВА ГЕНЕРАЦІЯ ТА ВСТАВКА ---
        let entity_generator = EntityGenerator::new();
        let mut generated_pks: DataPools = HashMap::new();
        
        let graph = self.build_dependency_graph(&architectural_plan);
        let sorted_entities = toposort(&graph, None).map_err(|_| AppError::CyclicDependency)?;

        println!("\n🚀 Порядок заповнення сутностей визначено:");
        for (i, entity_name) in sorted_entities.iter().enumerate() {
            println!("   {}. {}", i + 1, style(entity_name).cyan());
        }

        for entity_name in sorted_entities {
            if let Some(entity_template) = architectural_plan.entity_templates.iter().find(|e| e.entity_name == *entity_name) {
                if let Some(task) = plan.iter().find(|t| t.table == entity_template.target_table) {
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
        pks: &DataPools,
    ) -> AppResult<Vec<Value>> {
        let bar = ProgressBar::new(task.rows as u64);
        let mut generated_pks_for_this_table = Vec::new();
        
        let table_schema = self.schema.tables.get(&template.target_table)
            .ok_or_else(|| AppError::Custom(format!("Схема для таблиці '{}' не знайдена", template.target_table)))?;

        // Знаходимо опис первинного ключа
        let pk_field = template.fields.iter().find(|f| f.generator == "pk_hash" || f.generator == "pk_uuid");

        let mut tx = self.db_client.pool().begin().await?;
        for _ in 0..task.rows {
            let entity = generator.generate_entity(&template.fields, pools, pks)?;

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

            if let Some(pk_field) = pk_field {
                sql.push_str(&format!(" RETURNING \"{}\"", pk_field.column_name));
            }
            
            let mut query = sqlx::query(&sql);
            for val in values {
                match val {
                    Value::String(s) => { query = query.bind(s); }
                    Value::Number(n) => {
                        if let Some(i) = n.as_i64() { query = query.bind(i); }
                        else if let Some(f) = n.as_f64() { query = query.bind(f); }
                        else { query = query.bind(n.to_string()); }
                    }
                    Value::Bool(b) => {
                        let int_val: i32 = if b { 1 } else { 0 };
                        query = query.bind(int_val);
                    }
                    Value::Null => return Err(AppError::Custom("Спроба прив'язати NULL.".to_string())),
                    _ => { query = query.bind(val); }
                }
            }
            
            // ВИПРАВЛЕНО: Фінальна, надійна логіка читання PK
            if let Some(pk_field_template) = pk_field {
                let row = query.fetch_one(&mut *tx).await?;

                // Знаходимо схему для первинного ключа, щоб дізнатися його тип
                let pk_col_schema = table_schema.columns.iter().find(|c| c.name == pk_field_template.column_name)
                    .ok_or_else(|| AppError::Custom(format!("Не знайдено схему для PK колонки {}", pk_field_template.column_name)))?;

                // Універсально читаємо PK в залежності від його типу даних
                let pk_val: Value = match pk_col_schema.data_type.as_str() {
                    "character varying" | "text" | "varchar" | "uuid" => {
                        let val: String = row.get(0); // Читаємо як рядок
                        Value::String(val)
                    },
                    "integer" | "smallint" => {
                        let val: i32 = row.get(0); // Читаємо як i32
                        json!(val)
                    },
                    "bigint" => {
                        let val: i64 = row.get(0); // Читаємо як i64
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

    fn build_dependency_graph<'a>(&self, plan: &'a ArchitecturalPlan) -> DiGraphMap<&'a str, ()> {
        let mut graph: DiGraphMap<&'a str, ()> = DiGraphMap::new();

        // Додаємо всі сутності як вузли
        for template in &plan.entity_templates {
            graph.add_node(template.entity_name.as_str());
        }
        
        // Додаємо ребра на основі FK
        for template in &plan.entity_templates {
            for field in &template.fields {
                if field.generator == "fk" {
                    if let Some(parent_table_name) = field.params.get("references").and_then(|v| v.as_str()) {
                        // Знаходимо батьківську сутність за назвою таблиці
                        if let Some(parent_template) = plan.entity_templates.iter().find(|e| e.target_table == parent_table_name) {
                            graph.add_edge(
                                parent_template.entity_name.as_str(),
                                template.entity_name.as_str(),
                                (),
                            );
                        }
                    }
                }
            }
        }
        graph
    }
}