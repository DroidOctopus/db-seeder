// src/main.rs
mod config;
mod db;
mod entity_generator;
mod error;
mod gemini_analyzer;
mod interactive;
mod seeder;

use crate::config::AppConfig;
use crate::db::DbClient;
use crate::error::AppResult;
use crate::seeder::Seeder;
use clap::{Parser, Subcommand};
use console::style;

#[derive(Parser, Debug)]
#[command(author, version, about = "Утиліта для інтелектуального заповнення БД", long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Commands,

    #[arg(short, long, global = true, default_value = "config.toml")]
    config: String,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Заповнити БД на основі плану з файлу конфігурації (режим 'Архітектор')
    File,
    /// Запустити інтерактивний режим для вибору таблиць
    Interactive,
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
    // Робимо конфігурацію мутабельною, щоб можна було змінити `plan`
    let mut config = AppConfig::from_file(&cli.config)?;

    println!("🔌 Підключаюся до бази даних...");
    let db_client = DbClient::new(&config.database.url).await?;
    println!("✅ Підключення успішне.");

    let seeder = Seeder::new(db_client).await?;

    match cli.command {
        Commands::File => {
            println!("\n▶️  Режим: заповнення з файлу.");
            seeder.run(&config).await?;
        }
        Commands::Interactive => {
            println!("\n▶️  Режим: інтерактивний.");
            let default_rows = config.default_rows.unwrap_or(10);
            
            // Викликаємо правильну функцію
            let graph = seeder.build_full_dependency_graph();
            let plan = interactive::run_interactive_mode(seeder.schema(), &graph, default_rows)?;

            if !plan.is_empty() {
                // Оновлюємо план в існуючій конфігурації
                config.plan = Some(plan);
                seeder.run(&config).await?;
            }
        }
    }

    Ok(())
}