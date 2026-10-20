use dioxus::prelude::*;

use crate::api::users;
use crate::components::ui::button::Button;
use crate::components::ui::card::*;
use crate::components::ui::loading::Spinner;
use crate::components::ui::toast::{ToastVariant, show_toast};
use crate::types::CreateUserRequest;

#[derive(Debug, Clone, Default)]
struct ImportLine {
    id: String,
    displayname: String,
    password: String,
    admin: bool,
    deactivated: bool,
}

#[derive(Debug, Clone, Default)]
struct ImportStats {
    total: usize,
    admin_count: usize,
    with_password: usize,
    created: usize,
    skipped: usize,
    errors: Vec<String>,
}

#[component]
pub fn UserImport(on_import_complete: EventHandler<()>) -> Element {
    let mut csv_content = use_signal(String::new);
    let mut parsed_lines = use_signal(Vec::<ImportLine>::new);
    let mut stats = use_signal(ImportStats::default);
    let mut importing = use_signal(|| false);
    let mut import_done = use_signal(|| false);

    let handle_file_input = move |evt: FormEvent| {
        let value = evt.value();
        csv_content.set(value.clone());
        let lines = parse_csv(&value);
        let s = ImportStats {
            total: lines.len(),
            admin_count: lines.iter().filter(|l| l.admin).count(),
            with_password: lines.iter().filter(|l| !l.password.is_empty()).count(),
            ..ImportStats::default()
        };
        stats.set(s);
        parsed_lines.set(lines);
    };

    let handle_import = move |_: MouseEvent| {
        let lines = parsed_lines.read().clone();
        if lines.is_empty() {
            show_toast("No users to import", ToastVariant::Error);
            return;
        }

        importing.set(true);
        // With Pasion the admin flag is owned by the Pasion account (palpo
        // refuses local changes), so the `admin` column is ignored.
        let has_pasion = crate::utils::storage::get_item("pasion_url").is_some();

        spawn(async move {
            let mut created = 0usize;
            let mut skipped = 0usize;
            let mut errors = Vec::new();

            for line in lines.iter() {
                let password = if line.password.is_empty() {
                    crate::utils::password::generate_random_password()
                } else {
                    line.password.clone()
                };

                let request = CreateUserRequest {
                    password: Some(password),
                    displayname: if line.displayname.is_empty() {
                        None
                    } else {
                        Some(line.displayname.clone())
                    },
                    admin: !has_pasion && line.admin,
                    deactivated: Some(line.deactivated),
                    ..Default::default()
                };

                match users::create_user(&line.id, request).await {
                    Ok(_) => created += 1,
                    Err(e) => {
                        if e.message.contains("User ID already taken")
                            || e.message.contains("M_USER_IN_USE")
                        {
                            skipped += 1;
                        } else {
                            errors.push(format!("{}: {}", line.id, e.message));
                        }
                    }
                }
            }

            let mut s = stats.peek().clone();
            s.created = created;
            s.skipped = skipped;
            s.errors = errors;
            stats.set(s);
            importing.set(false);
            import_done.set(true);

            show_toast(
                &format!("Import complete: {created} created, {skipped} skipped"),
                ToastVariant::Success,
            );

            if created > 0 {
                on_import_complete.call(());
            }
        });
    };

    let is_importing = *importing.read();
    let is_done = *import_done.read();
    let current_stats = stats.read().clone();
    let has_lines = !parsed_lines.read().is_empty();

    rsx! {
        div { class: "space-y-6",
            // Upload section
            Card {
                CardHeader {
                    CardTitle { "Import Users from CSV" }
                    CardDescription {
                        "Paste CSV content with columns: id, displayname, password, admin, deactivated"
                    }
                }
                CardContent { class: "space-y-4".to_string(),
                    textarea {
                        class: "flex min-h-[200px] w-full rounded-md border border-input bg-background px-3 py-2 text-sm ring-offset-background placeholder:text-muted-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2 font-mono",
                        placeholder: "id,displayname,password,admin\nuser1,User One,password123,false\nuser2,User Two,,true",
                        value: csv_content.read().clone(),
                        oninput: handle_file_input,
                    }
                    p { class: "text-xs text-muted-foreground",
                        "Required columns: id (or displayname). Optional: password, admin (true/false), deactivated (true/false)"
                    }
                }
            }

            // Stats
            if has_lines {
                Card {
                    CardHeader {
                        CardTitle { "Import Preview" }
                    }
                    CardContent {
                        div { class: "grid gap-4 md:grid-cols-3",
                            div { class: "text-center p-4 rounded-md bg-muted",
                                div { class: "text-2xl font-bold", "{current_stats.total}" }
                                p { class: "text-xs text-muted-foreground", "Total Users" }
                            }
                            div { class: "text-center p-4 rounded-md bg-muted",
                                div { class: "text-2xl font-bold", "{current_stats.admin_count}" }
                                p { class: "text-xs text-muted-foreground", "Admins" }
                            }
                            div { class: "text-center p-4 rounded-md bg-muted",
                                div { class: "text-2xl font-bold", "{current_stats.with_password}" }
                                p { class: "text-xs text-muted-foreground", "With Password" }
                            }
                        }
                    }
                    CardFooter {
                        Button {
                            disabled: is_importing || is_done,
                            onclick: handle_import,
                            if is_importing {
                                Spinner { class: "mr-2".to_string() }
                                "Importing..."
                            } else if is_done {
                                "Import Complete"
                            } else {
                                "Start Import"
                            }
                        }
                    }
                }
            }

            // Results
            if is_done {
                Card {
                    CardHeader {
                        CardTitle { "Import Results" }
                    }
                    CardContent {
                        div { class: "space-y-2",
                            p { class: "text-sm",
                                span { class: "font-medium text-green-600", "{current_stats.created}" }
                                " users created"
                            }
                            if current_stats.skipped > 0 {
                                p { class: "text-sm",
                                    span { class: "font-medium text-yellow-600", "{current_stats.skipped}" }
                                    " users skipped (already exist)"
                                }
                            }
                            if !current_stats.errors.is_empty() {
                                div { class: "mt-4",
                                    p { class: "text-sm font-medium text-destructive mb-2",
                                        "{current_stats.errors.len()} errors:"
                                    }
                                    for error in current_stats.errors.iter() {
                                        p { class: "text-xs text-destructive", "{error}" }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

fn parse_csv(content: &str) -> Vec<ImportLine> {
    let mut lines = Vec::new();
    let mut iter = content.lines();

    // Parse header
    let header = match iter.next() {
        Some(h) => h,
        None => return lines,
    };

    let columns: Vec<&str> = header.split(',').map(|s| s.trim()).collect();

    let id_idx = columns.iter().position(|c| *c == "id");
    let name_idx = columns.iter().position(|c| *c == "displayname");
    let pass_idx = columns.iter().position(|c| *c == "password");
    let admin_idx = columns.iter().position(|c| *c == "admin");
    let deactivated_idx = columns.iter().position(|c| *c == "deactivated");

    for line in iter {
        let fields: Vec<&str> = line.split(',').map(|s| s.trim()).collect();
        if fields.is_empty() || (fields.len() == 1 && fields[0].is_empty()) {
            continue;
        }

        let id = id_idx
            .and_then(|i| fields.get(i))
            .map(|s| s.to_string())
            .unwrap_or_default();

        if id.is_empty() {
            continue;
        }

        let displayname = name_idx
            .and_then(|i| fields.get(i))
            .map(|s| s.to_string())
            .unwrap_or_default();

        let password = pass_idx
            .and_then(|i| fields.get(i))
            .map(|s| s.to_string())
            .unwrap_or_default();

        let admin = admin_idx
            .and_then(|i| fields.get(i))
            .map(|s| *s == "true" || *s == "1")
            .unwrap_or(false);

        let deactivated = deactivated_idx
            .and_then(|i| fields.get(i))
            .map(|s| *s == "true" || *s == "1")
            .unwrap_or(false);

        lines.push(ImportLine {
            id,
            displayname,
            password,
            admin,
            deactivated,
        });
    }

    lines
}
