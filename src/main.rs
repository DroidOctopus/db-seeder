// src/main.rs
mod config;
mod db;
mod entity_generator;
mod error;
mod gemini_analyzer;
mod seeder;

use crate::config::AppConfig;
use crate::db::DbClient;
use crate::error::AppResult;
use crate::seeder::Seeder;
use clap::Parser;
use console::style;

#[derive(Parser, Debug)]
#[command(author, version, about = "Утиліта для заповнення БД в режимі 'Gemini як архітектор'", long_about = None)]
struct Cli {
    /// Шлях до файлу конфігурації
    #[arg(short, long, default_value = "config.toml")]
    config: String,
}

#[tokio::main]
async fn main() -> AppResult<()> {
    if let Err(e) = dotenvy::dotenv() {
        if !e.to_string().contains("No such file or directory") {
            eprintln!("{} Помилка завантаження .env файлу: {}", style("[!]").yellow(), e);
        }
    }

    let cli = Cli::parse();

    println!("⚙️  Завантажую конфігурацію з '{}'...", &cli.config);
    let config = AppConfig::from_file(&cli.config)?;

    println!("🔌 Підключаюся до бази даних...");
    let db_client = DbClient::new(&config.database.url).await?;
    println!("✅ Підключення успішне.");

    // ВИПРАВЛЕНО: Створення Seeder з одним аргументом
    let seeder = Seeder::new(db_client).await?;

    println!("\n▶️  Запускаю заповнення на основі плану з конфігурації...");
    if config.plan.is_some() {
        seeder.run(&config).await?;
    } else {
        println!("{}", style("У файлі конфігурації не знайдено секції [[seeding_plan]].").yellow());
    }

    Ok(())
}