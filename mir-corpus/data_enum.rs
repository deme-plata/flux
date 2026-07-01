// MIR-drift corpus — data-carrying enum (ladder rung 4)
// Tests: construction, discriminant, match with payload, downcast
pub enum Opt {
    None,
    Some(i64),
}

pub fn make_some(x: i64) -> Opt {
    Opt::Some(x)
}

pub fn make_none() -> Opt {
    Opt::None
}

pub fn is_some(o: Opt) -> i64 {
    match o {
        Opt::Some(v) => v,
        Opt::None => -1,
    }
}

pub fn get_discriminant(o: Opt) -> i64 {
    if matches!(o, Opt::Some(_)) { 1 } else { 0 }
}
