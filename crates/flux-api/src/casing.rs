// Case helpers shared by the SDK generators. Private to the crate so v0.13's
// macro work can swap them out without touching the public surface.

pub(crate) fn pascal_case(s: &str) -> String {
    s.split('_')
        .map(|w| {
            let mut c = w.chars();
            match c.next() {
                None => String::new(),
                Some(f) => f.to_uppercase().collect::<String>() + &c.collect::<String>(),
            }
        })
        .collect()
}

pub(crate) fn camel_case(s: &str) -> String {
    let p = pascal_case(s);
    let mut c = p.chars();
    match c.next() {
        None => String::new(),
        Some(f) => f.to_lowercase().collect::<String>() + &c.collect::<String>(),
    }
}

pub(crate) fn snake_case(s: &str) -> String {
    s.replace('-', "_").to_lowercase()
}
