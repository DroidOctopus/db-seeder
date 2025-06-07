// src/db.rs
use sqlx::{postgres::PgRow, Pool, Postgres, Row};
use std::collections::HashMap;

use crate::error::{AppError, AppResult};

// --- Структури для опису схеми БД ---

#[derive(Debug, Clone)]
pub struct ColumnSchema {
    pub name: String,
    pub data_type: String,
    pub is_nullable: bool,
    pub column_default: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ForeignKey {
    pub from_table: String,
    pub from_column: String,
    pub to_table: String,
    pub to_column: String,
}

#[derive(Debug, Clone)]
pub struct TableSchema {
    pub name: String,
    pub columns: Vec<ColumnSchema>,
    pub primary_key_column: Option<String>,
}

pub struct DbSchema {
    pub tables: HashMap<String, TableSchema>,
    pub foreign_keys: Vec<ForeignKey>,
}

// --- Клієнт для роботи з БД ---

pub struct DbClient {
    pool: Pool<Postgres>,
}

impl DbClient {
    /// Створює новий екземпляр клієнта та підключається до БД
    pub async fn new(db_url: &str) -> AppResult<Self> {
        let pool = Pool::<Postgres>::connect(db_url).await?;
        Ok(Self { pool })
    }

    pub fn pool(&self) -> &Pool<Postgres> {
        &self.pool
    }

    /// Отримує повну схему бази даних (таблиці, колонки, зв'язки)
    pub async fn fetch_schema(&self) -> AppResult<DbSchema> {
        // Отримуємо всі таблиці
        let table_rows = sqlx::query(
            "SELECT table_name FROM information_schema.tables WHERE table_schema = 'public' AND table_type = 'BASE TABLE'",
        )
        .fetch_all(&self.pool)
        .await?;

        let mut tables = HashMap::new();
        for row in table_rows {
            let table_name: String = row.get("table_name");
            let columns = self.fetch_columns_for_table(&table_name).await?;
            let primary_key_column = self.fetch_primary_key(&table_name).await?;
            tables.insert(
                table_name.clone(),
                TableSchema {
                    name: table_name,
                    columns,
                    primary_key_column,
                },
            );
        }

        let foreign_keys = self.fetch_foreign_keys().await?;

        Ok(DbSchema {
            tables,
            foreign_keys,
        })
    }
    
    /// Отримує колонки для конкретної таблиці
    async fn fetch_columns_for_table(&self, table_name: &str) -> AppResult<Vec<ColumnSchema>> {
        // ВИПРАВЛЕНО: Додаємо `column_default` до запиту
        let rows = sqlx::query(
            "SELECT column_name, data_type, is_nullable, column_default 
             FROM information_schema.columns 
             WHERE table_name = $1 AND table_schema = 'public'"
        )
        .bind(table_name)
        .fetch_all(&self.pool)
        .await?;

        Ok(rows.into_iter().map(|row| ColumnSchema {
            name: row.get("column_name"),
            data_type: row.get("data_type"),
            is_nullable: row.get::<String, _>("is_nullable") == "YES",
            // ВИПРАВЛЕНО: Читаємо значення за замовчуванням
            column_default: row.get("column_default"),
        }).collect())
    }
    
    /// Отримує первинний ключ таблиці (підтримуємо тільки один для простоти)
    async fn fetch_primary_key(&self, table_name: &str) -> AppResult<Option<String>> {
        let row: Option<PgRow> = sqlx::query(r#"
            SELECT a.attname
            FROM   pg_index i
            JOIN   pg_attribute a ON a.attrelid = i.indrelid
                                AND a.attnum = ANY(i.indkey)
            WHERE  i.indrelid = $1::regclass
            AND    i.indisprimary;
        "#)
        .bind(table_name)
        .fetch_optional(&self.pool)
        .await?;
        
        Ok(row.map(|r| r.get("attname")))
    }
    
    /// Отримує всі зовнішні ключі в схемі
    async fn fetch_foreign_keys(&self) -> AppResult<Vec<ForeignKey>> {
        let rows = sqlx::query(r#"
            SELECT
                tc.table_name AS from_table,
                kcu.column_name AS from_column,
                ccu.table_name AS to_table,
                ccu.column_name AS to_column
            FROM
                information_schema.table_constraints AS tc
                JOIN information_schema.key_column_usage AS kcu
                  ON tc.constraint_name = kcu.constraint_name
                  AND tc.table_schema = kcu.table_schema
                JOIN information_schema.constraint_column_usage AS ccu
                  ON ccu.constraint_name = tc.constraint_name
                  AND ccu.table_schema = tc.table_schema
            WHERE tc.constraint_type = 'FOREIGN KEY' AND tc.table_schema = 'public';
        "#)
        .fetch_all(&self.pool)
        .await?;

        Ok(rows.into_iter().map(|row| ForeignKey {
            from_table: row.get("from_table"),
            from_column: row.get("from_column"),
            to_table: row.get("to_table"),
            to_column: row.get("to_column"),
        }).collect())
    }
}