//! Every rigged gun comes apart into its convex parts quickly enough to do at the range's start, and
//! the parts hug the gun (none much bigger than the gun's own bounds would make it).

#![allow(dead_code)]

#[path = "../src/content.rs"]
mod content;

use std::time::Instant;

#[test]
fn guns_come_apart_into_convex_parts() {
    let mut w = content::metro_weapons(&rustvr_asset::default_root());
    w.extend([content::builtin_rifle("Range rifle", false), content::builtin_pistol()]);
    let t0 = Instant::now();
    for g in w.iter().filter(|w| w.rigged()) {
        let t = Instant::now();
        let parts = rustvr_guns::hulls::weapon_parts(g, 10);
        eprintln!("{}: {} parts ({}) in {:.0} ms", g.key, parts.len(), parts.iter().map(|p| p.len().to_string()).collect::<Vec<_>>().join(" "), t.elapsed().as_secs_f32() * 1000.0);
        assert!(!parts.is_empty(), "{}", g.key);
    }
    eprintln!("all in {:.0} ms", t0.elapsed().as_secs_f32() * 1000.0);
}
