const WIDTH: usize = 68;

pub fn show(text: &[u8]) {
    let text = String::from_utf8_lossy(text);
    let text = text.strip_prefix('\u{feff}').unwrap_or(&text);
    let mut paragraph = Vec::new();
    for line in text.split('\n').chain([""]) {
        let heading = match line.strip_prefix("# ") {
            Some(text) => Some((text, '=')),
            None => line.strip_prefix("## ").map(|t| (t, '-')),
        };
        let blank = line.chars().all(|c| " \t\r".contains(c));
        if heading.is_none() && !blank {
            paragraph.push(line);
            continue;
        }
        if !paragraph.is_empty() {
            block(&paragraph.join(" "), None);
            paragraph.clear();
        }
        if let Some((text, rule)) = heading {
            block(text, Some(rule));
        }
    }
}

fn block(text: &str, rule: Option<char>) {
    let mut lines = wrap(text);
    if let Some(rule) = rule {
        let n = lines.join(" ").chars().count().min(WIDTH);
        lines.push(rule.to_string().repeat(n));
    }
    println!("{}\n", lines.join("\n"));
}

fn wrap(text: &str) -> Vec<String> {
    let mut lines = vec![String::new()];
    let text = visible(text);
    for word in text.split(' ').filter(|w| !w.is_empty()) {
        let line = lines.last_mut().unwrap();
        let n = line.chars().count() + 1 + word.chars().count();
        if line.is_empty() {
            line.push_str(word);
        } else if n > WIDTH {
            lines.push(word.to_string());
        } else {
            line.push(' ');
            line.push_str(word);
        }
    }
    lines
}

pub fn visible(text: &str) -> String {
    text.chars()
        .filter_map(|c| match c {
            '\t' | '\r' => Some(' '),
            c if c.is_control() => None,
            c => Some(c),
        })
        .collect()
}
