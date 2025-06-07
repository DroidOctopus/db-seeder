// src/error.rs
use thiserror::Error;

// Уніфікований тип помилок для всього додатку для зручної обробки
#[derive(Error, Debug)]
pub enum AppError {
    #[error("Помилка конфігурації: {0}")]
    Config(#[from] config::ConfigError),

    #[error("Помилка вводу/виводу: {0}")]
    Io(#[from] std::io::Error),

    #[error("Помилка бази даних: {0}")]
    Db(#[from] sqlx::Error),

    #[error("Помилка під час API запиту: {0}")]
    Api(#[from] reqwest::Error),

    #[error("Помилка серіалізації/десеріалізації JSON: {0}")]
    Json(#[from] serde_json::Error),

    #[error("Помилка змінних середовища: {0}")]
    DotEnv(#[from] dotenvy::Error),

    #[error("Помилка інтерактивного режиму: {0}")]
    Dialoguer(#[from] dialoguer::Error),

    #[error("Помилка шаблону прогрес-бару: {0}")]
    Progress(#[from] indicatif::style::TemplateError),

    #[error("Не вдалося знайти залежність для таблиці '{0}'")]
    DependencyNotFound(String),

    #[error("Знайдено циклічну залежність в схемі БД, заповнення неможливе")]
    CyclicDependency,

    #[error("Інтерактивну сесію було перервано")]
    Interrupted,

    #[error("Невідомий генератор даних: {0}")]
    UnknownGenerator(String),

    #[error("Кастомна помилка: {0}")]
    Custom(String),
}

// Створюємо псевдонім для Result для зручності
pub type AppResult<T> = Result<T, AppError>;