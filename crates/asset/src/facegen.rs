//! FaceGen's controls of a face, as a game's `FaceGen\si.ctl` has them (Oblivion's; Fallout 3's and
//! New Vegas's are the same format): the named controls a character creator's sliders move (each a
//! unit direction in the symmetric geometry coordinates, its value the coordinates' dot with it),
//! and the face's age and gender, each a linear function of the same coordinates. Written by an
//! extractor as `facegen.toml` in the game's cache folder.
//!
//! The arithmetic of moving a face within them: setting some controls to values while others stay
//! as they are ([`FaceControls::set`]), and keeping a face inside the sliders' ranges
//! ([`FaceControls::limit`]).

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::Path;

pub const FACEGEN_FILE: &str = "facegen.toml";

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct FaceControls {
    /// The age from the symmetric geometry coordinates: `Σ age[i]·gs[i] + age_offset`.
    pub age: Vec<f32>,
    pub age_offset: f32,
    /// The gender, the same way (FaceGen's gender: its race menus' "complexion").
    #[serde(default)]
    pub gender: Vec<f32>,
    #[serde(default)]
    pub gender_offset: f32,
    /// The named controls over the symmetric geometry, in the file's order (a game's sliders name
    /// them by their place in it).
    #[serde(default)]
    pub controls: Vec<FaceControl>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct FaceControl {
    pub name: String,
    /// A unit direction in the symmetric geometry coordinates.
    pub coefficients: Vec<f32>,
}

/// The cache's FaceGen controls for a game.
pub fn load_face_controls(dir: &Path) -> Result<FaceControls> {
    let path = dir.join(FACEGEN_FILE);
    let text = std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
    toml::from_str(&text).with_context(|| format!("decoding {}", path.display()))
}

pub fn save_face_controls(dir: &Path, c: &FaceControls) -> Result<()> {
    std::fs::create_dir_all(dir)?;
    let path = dir.join(FACEGEN_FILE);
    std::fs::write(&path, toml::to_string(c)?).with_context(|| format!("writing {}", path.display()))
}

fn dot(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

impl FaceControls {
    /// FaceGen's age of a face (its symmetric geometry coordinates).
    pub fn age_of(&self, gs: &[f32]) -> f32 {
        dot(&self.age, gs) + self.age_offset
    }

    /// Control `i`'s value for a face.
    pub fn value(&self, i: usize, gs: &[f32]) -> f32 {
        self.controls.get(i).map_or(0.0, |c| dot(&c.coefficients, gs))
    }

    /// The directions a face keeps when it's reshaped by hand: its age and its gender (a race menu
    /// has its own sliders for them, and the age picks the face's age map).
    pub fn kept(&self) -> Vec<&[f32]> {
        [&self.age, &self.gender].into_iter().filter(|v| !v.is_empty()).map(Vec::as_slice).collect()
    }

    /// Sets the controls `targets` ((index, value)) on the face `gs` with the least change to its
    /// coordinates, its age and gender kept: the race menu's own sliders set one control at a time
    /// along its direction, which moves the others it isn't square to; this sets them together.
    pub fn set(&self, gs: &mut [f32], targets: &[(usize, f32)]) {
        let mut rows: Vec<&[f32]> = Vec::new();
        let mut want: Vec<f64> = Vec::new();
        for &(i, v) in targets {
            if let Some(c) = self.controls.get(i) {
                rows.push(&c.coefficients);
                want.push((v - dot(&c.coefficients, gs)) as f64);
            }
        }
        for k in self.kept() {
            rows.push(k);
            want.push(0.0);
        }
        let d = least_change(&rows, &want, gs.len());
        for (g, d) in gs.iter_mut().zip(d) {
            *g += d;
        }
    }

    /// Keeps the face `gs` within the controls' ranges (`limits[i]`: control `i`'s value at most that
    /// far either side of 0; 0 or missing: no limit), its age and gender as they were in `from`:
    /// each control past its range is brought back along its own direction, and those moves are taken
    /// off the age and gender again, until it all holds (or near enough, after a few rounds).
    pub fn limit(&self, gs: &mut [f32], limits: &[f32], from: &[f32]) {
        let kept = self.kept();
        let targets: Vec<f32> = kept.iter().map(|k| dot(k, from)).collect();
        for _ in 0..16 {
            let mut moved = false;
            for (c, &l) in self.controls.iter().zip(limits) {
                if l <= 0.0 {
                    continue;
                }
                let v = dot(&c.coefficients, gs);
                let over = v - v.clamp(-l, l);
                if over != 0.0 {
                    moved = true;
                    for (g, k) in gs.iter_mut().zip(&c.coefficients) {
                        *g -= over * k;
                    }
                }
            }
            let want: Vec<f64> = kept.iter().zip(&targets).map(|(k, t)| (t - dot(k, gs)) as f64).collect();
            if want.iter().any(|w| w.abs() > 1e-5) {
                let d = least_change(&kept, &want, gs.len());
                for (g, d) in gs.iter_mut().zip(d) {
                    *g += d;
                }
            }
            if !moved {
                break;
            }
        }
    }
}

/// The smallest change `d` (of `n` coordinates) with `rows[r] · d = want[r]` for every row: `d =
/// Rᵀ (R Rᵀ)⁻¹ want`. Rows that depend on others are let go (their part of `want` as far as it can
/// be met).
pub fn least_change(rows: &[&[f32]], want: &[f64], n: usize) -> Vec<f32> {
    let m = rows.len();
    if m == 0 {
        return vec![0.0; n];
    }
    let mut g = vec![0f64; m * m];
    for i in 0..m {
        for j in 0..m {
            g[i * m + j] = rows[i].iter().zip(rows[j]).map(|(a, b)| *a as f64 * *b as f64).sum();
        }
        // (A touch on the diagonal, so rows that depend on others don't blow it up.)
        g[i * m + i] += 1e-9;
    }
    let mut x = want.to_vec();
    if !solve(&mut g, &mut x, m) {
        return vec![0.0; n];
    }
    let mut d = vec![0f32; n];
    for (r, xr) in rows.iter().zip(&x) {
        for (dk, rk) in d.iter_mut().zip(r.iter()) {
            *dk += (*rk as f64 * xr) as f32;
        }
    }
    d
}

/// Solves `a x = b` in place (`a` n×n, row-major, `b` becomes `x`) by Gaussian elimination with
/// partial pivoting; false when `a` is singular.
pub fn solve(a: &mut [f64], b: &mut [f64], n: usize) -> bool {
    for col in 0..n {
        let pivot = (col..n).max_by(|&i, &j| a[i * n + col].abs().total_cmp(&a[j * n + col].abs())).unwrap_or(col);
        if a[pivot * n + col].abs() < 1e-14 {
            return false;
        }
        if pivot != col {
            for k in 0..n {
                a.swap(col * n + k, pivot * n + k);
            }
            b.swap(col, pivot);
        }
        for row in col + 1..n {
            let f = a[row * n + col] / a[col * n + col];
            if f == 0.0 {
                continue;
            }
            for k in col..n {
                a[row * n + k] -= f * a[col * n + k];
            }
            b[row] -= f * b[col];
        }
    }
    for col in (0..n).rev() {
        let mut s = b[col];
        for k in col + 1..n {
            s -= a[col * n + k] * b[k];
        }
        b[col] = s / a[col * n + col];
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unit(n: usize, v: &[(usize, f32)]) -> Vec<f32> {
        let mut out = vec![0.0; n];
        for &(i, x) in v {
            out[i] = x;
        }
        let l = out.iter().map(|x| x * x).sum::<f32>().sqrt();
        out.iter().map(|x| x / l).collect()
    }

    fn controls() -> FaceControls {
        FaceControls {
            age: unit(4, &[(0, 1.0)]).iter().map(|x| x * 3.0).collect(),
            age_offset: 30.0,
            gender: Vec::new(),
            gender_offset: 0.0,
            controls: vec![
                FaceControl { name: "a".into(), coefficients: unit(4, &[(1, 1.0)]) },
                // Not square to the first.
                FaceControl { name: "b".into(), coefficients: unit(4, &[(1, 1.0), (2, 1.0)]) },
                FaceControl { name: "c".into(), coefficients: unit(4, &[(0, 1.0), (3, 1.0)]) },
            ],
        }
    }

    #[test]
    fn setting_controls_sets_them_together_and_keeps_the_age() {
        let c = controls();
        let mut gs = vec![0.5, 0.2, -0.3, 0.1];
        let age = c.age_of(&gs);
        c.set(&mut gs, &[(0, 1.0), (1, -0.5)]);
        assert!((c.value(0, &gs) - 1.0).abs() < 1e-4);
        assert!((c.value(1, &gs) + 0.5).abs() < 1e-4);
        assert!((c.age_of(&gs) - age).abs() < 1e-4);
    }

    #[test]
    fn a_face_is_kept_in_the_sliders_ranges() {
        let c = controls();
        let from = vec![0.0, 0.0, 0.0, 0.0];
        let mut gs = vec![0.0, 4.0, 1.0, 2.0];
        c.limit(&mut gs, &[2.0, 2.0, 1.0], &from);
        for (i, l) in [2.0, 2.0, 1.0].into_iter().enumerate() {
            assert!(c.value(i, &gs).abs() <= l + 1e-3, "control {i} is {}", c.value(i, &gs));
        }
        assert!((c.age_of(&gs) - c.age_of(&from)).abs() < 1e-3);
    }

    #[test]
    fn the_least_change_meets_its_rows() {
        let a = [1.0f32, 0.0, 0.0];
        let b = [0.0f32, 1.0, 1.0];
        let d = least_change(&[&a, &b], &[2.0, 4.0], 3);
        assert!((d[0] - 2.0).abs() < 1e-5 && (d[1] - 2.0).abs() < 1e-5 && (d[2] - 2.0).abs() < 1e-5);
    }
}
