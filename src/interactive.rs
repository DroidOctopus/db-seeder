// src/interactive.rs
use crate::config::SeedingTask;
use crate::db::DbSchema;
use crate::error::{AppError, AppResult};
use console::{style, Term};
use dialoguer::theme::Theme;
use dialoguer::Input;
use petgraph::algo::has_path_connecting;
use petgraph::graphmap::DiGraphMap;
use std::collections::HashSet;

/// Спеціальна тема для нашого меню, щоб додати індикатори
struct CustomTheme;

impl Theme for CustomTheme {
    fn format_select_prompt_item(
        &self,
        f: &mut dyn std::fmt::Write,
        text: &str,
        active: bool,
    ) -> std::fmt::Result {
        let (prefix, text) = text.split_at(4); // Розділяємо "[*] " або "[ ] "
        if active {
            write!(f, "{} {}", style(prefix).cyan().bright(), style(text).cyan())
        } else {
            write!(f, "{} {}", style(prefix).dim(), style(text).dim())
        }
    }

    fn format_input_prompt(
        &self,
        f: &mut dyn std::fmt::Write,
        prompt: &str,
        default: Option<&str>,
    ) -> std::fmt::Result {
        if let Some(default) = default {
            write!(f, "{_green}?{_reset} {} {_dim}({default}){_reset} ", prompt, _green = "\x1b[32m", _reset = "\x1b[0m", _dim = "\x1b[2m")
        } else {
            write!(f, "{_green}?{_reset} {} ", prompt, _green = "\x1b[32m", _reset = "\x1b[0m")
        }
    }
}

pub fn run_interactive_mode(
    schema: &DbSchema,
    graph: &DiGraphMap<&str, ()>,
    default_rows: u32,
) -> AppResult<Vec<SeedingTask>> {
    let term = Term::stdout();
    let theme = CustomTheme;
    let mut selections = HashSet::new();
    let mut table_names: Vec<&str> = schema.tables.keys().map(|s| s.as_str()).collect();
    table_names.sort_unstable(); // Сортуємо для стабільного порядку

    loop {
        term.clear_screen()?;
        println!("{}", style("Інтерактивний вибір таблиць для заповнення:").bold());
        println!("(використовуйте ↑/↓, 'пробіл' для вибору, 'enter' для продовження, 'q' для виходу)\n");

        let mut items = Vec::new();
        for (_i, &table_name) in table_names.iter().enumerate() {
            let prefix = if selections.contains(table_name) { "[*]" } else { "[ ]" };
            
            // Шукаємо залежності (батьків)
            let parents: Vec<_> = graph.neighbors_directed(table_name, petgraph::Direction::Incoming).collect();
            let parent_str = if parents.is_empty() {
                "".to_string()
            } else {
                format!(" (залежить від: {})", parents.join(", "))
            };

            items.push(format!("{} {}{}", prefix, style(table_name).green(), style(parent_str).yellow()));
        }

        let selection = dialoguer::Select::with_theme(&theme)
            .items(&items)
            .default(0)
            .interact_on_opt(&term)?
            .ok_or(AppError::Interrupted)?;

        let selected_table = table_names[selection];

        // Логіка вибору/скасування вибору
        if selections.contains(selected_table) {
            // Скасування вибору (з перевіркою, чи хтось від нього не залежить)
            let is_dependency_for_others = selections.iter().any(|&other_table| {
                other_table != selected_table && has_path_connecting(graph, selected_table, other_table, None)
            });

            if is_dependency_for_others {
                 println!("\n{}", style("Неможливо скасувати вибір цієї таблиці, оскільки від неї залежать інші вибрані таблиці.").red());
                 std::thread::sleep(std::time::Duration::from_secs(2));
            } else {
                selections.remove(selected_table);
            }
        } else {
            // Вибір таблиці та всіх її залежностей
            selections.insert(selected_table);
            let mut to_visit = vec![selected_table];
            let mut visited = HashSet::new();
            
            while let Some(current) = to_visit.pop() {
                if visited.contains(current) { continue; }
                visited.insert(current);
                
                let parents = graph.neighbors_directed(current, petgraph::Direction::Incoming);
                for parent in parents {
                    selections.insert(parent);
                    to_visit.push(parent);
                }
            }
        }
        
        // Перевіряємо, чи користувач хоче завершити
        if dialoguer::Confirm::new()
            .with_prompt("Завершити вибір та перейти до налаштування кількості рядків?")
            .interact_on(&term)?
        {
            break;
        }
    }

    if selections.is_empty() {
        println!("Не вибрано жодної таблиці. Вихід.");
        return Ok(Vec::new());
    }

    println!("\n{}", style("Налаштування кількості рядків для вибраних таблиць:").bold());
    let mut plan = Vec::new();
    let dialoguer_theme = dialoguer::theme::ColorfulTheme::default();

    // Сортуємо вибрані таблиці для послідовного виводу
    let mut sorted_selections: Vec<_> = selections.into_iter().collect();
    sorted_selections.sort_unstable();

    for table_name in sorted_selections {
        let rows_str: String = Input::with_theme(&dialoguer_theme)
            .with_prompt(format!("Скільки рядків згенерувати для '{}'?", table_name))
            .default(default_rows.to_string()) // Передаємо рядок
            .interact_text()?; // Отримуємо рядок

        // Парсимо рядок, і якщо не виходить, беремо значення за замовчуванням
        let rows = rows_str.parse().unwrap_or(default_rows);

        plan.push(SeedingTask {
            table: table_name.to_string(),
            rows,
            ..Default::default()
        });
    }

    Ok(plan)
}