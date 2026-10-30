//! Recognises humanoid bones by name: 3ds Max Biped (`bip01_l_forearm`), Mixamo
//! (`mixamorig:LeftForeArm`), Unreal (`lowerarm_l`), Unity / VRM (`LeftLowerArm`, `J_Bip_L_LowerArm`),
//! Character Creator (`CC_Base_L_Forearm`) and Rigify (`forearm.L`).
//!
//! Names are split into lowercase words (at separators, case changes and digit runs), side words
//! (`l`, `left`, `r`, `right`) and rig prefixes are set aside, and what's left is matched against
//! keywords. Helpers (IK targets, sockets, end bones, Metro's `aux_` and `loc_` bones) are never
//! matched, so they follow their parent.

/// What a bone is, as far as its name tells.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Class {
    Hips,
    /// Any bone of the spine chain (spine, chest, upper chest).
    Spine,
    Neck,
    Head,
    Limb(usize, Limb),
    /// Side, finger (0 thumb .. 4 little), and whether it's a metacarpal (not a curling segment).
    Finger(usize, usize, bool),
    /// A twist (roll) helper along a limb segment.
    Twist,
    /// An eye (its side): turned to look where the eyes look.
    Eye(usize),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Limb {
    Shoulder,
    UpperArm,
    LowerArm,
    Hand,
    UpperLeg,
    LowerLeg,
    Foot,
    Toes,
}

/// Words that mark helper bones that must never drive (or be driven as) a body part.
const HELPER_WORDS: &[&str] = &[
    "end", "nub", "ik", "fk", "target", "pole", "ctrl", "control", "helper", "socket", "attach", "loc", "aux", "null", "dummy",
    "weapon", "prop", "jiggle", "corrective", "offset",
];

/// Rig prefixes and filler words that say nothing about the body part.
/// (`c` is VRoid's centre marker.)
const NOISE_WORDS: &[&str] = &["bip", "j", "c", "cc", "base", "def", "org", "mch", "b", "bn", "jnt", "joint", "bone", "mixamorig", "rig", "skel"];

pub fn classify(name: &str) -> Option<Class> {
    let words = split_words(name);
    if words.iter().any(|w| HELPER_WORDS.contains(&w.as_str())) {
        return None;
    }
    let mut side = None;
    let mut joined = String::new();
    for w in &words {
        match w.as_str() {
            "l" | "left" => side = Some(0),
            "r" | "right" => side = Some(1),
            _ if w.chars().all(|c| c.is_ascii_digit()) => {}
            _ if NOISE_WORDS.contains(&w.as_str()) => {}
            _ => joined.push_str(w),
        }
    }
    // Biped numbers its fingers: finger0 (thumb) .. finger4, with segments finger01, finger02.
    let finger_number = words
        .iter()
        .position(|w| w == "finger")
        .and_then(|i| words.get(i + 1))
        .and_then(|w| w.chars().next())
        .and_then(|c| c.to_digit(10))
        .filter(|&d| d < 5)
        .map(|d| d as usize);
    let has = |k: &str| joined.contains(k);
    if has("twist") || has("roll") {
        return Some(Class::Twist);
    }
    if let Some(side) = side {
        let finger = if has("thumb") {
            Some(0)
        } else if has("index") || has("pointer") {
            Some(1)
        } else if has("middle") {
            Some(2)
        } else if has("ring") {
            Some(3)
        } else if has("pinky") || has("little") {
            Some(4)
        } else if joined == "finger" {
            finger_number
        } else {
            None
        };
        if let Some(f) = finger {
            return Some(Class::Finger(side, f, has("meta") || has("carpal")));
        }
        // (The eye itself, as humanoid rigs name it: not its lid, brow or lashes, nor a game's own
        // eyeball helpers.)
        if joined == "eye" {
            return Some(Class::Eye(side));
        }
        let limb = if has("forearm") || has("lowerarm") || has("elbow") {
            Limb::LowerArm
        } else if has("arm") {
            Limb::UpperArm
        } else if has("hand") || has("wrist") || has("palm") {
            Limb::Hand
        } else if has("clavicle") || has("shoulder") || has("collar") {
            Limb::Shoulder
        } else if has("thigh") || has("upleg") || has("upperleg") || has("hip") {
            Limb::UpperLeg
        } else if has("calf") || has("shin") || has("lowerleg") || has("knee") || has("leg") {
            Limb::LowerLeg
        } else if has("foot") || has("ankle") {
            Limb::Foot
        } else if has("toe") || joined == "ball" {
            Limb::Toes
        } else {
            return None;
        };
        return Some(Class::Limb(side, limb));
    }
    if joined == "hips" || joined == "hip" || has("pelvis") {
        Some(Class::Hips)
    } else if has("spine") || has("chest") || has("torso") || has("abdomen") || has("waist") {
        Some(Class::Spine)
    } else if has("neck") {
        Some(Class::Neck)
    } else if words.iter().any(|w| w == "head") {
        Some(Class::Head)
    } else {
        None
    }
}

/// Lowercase words: split at non-alphanumerics, lower-to-upper case changes and letter/digit
/// boundaries. A namespace (`mixamorig:`) is dropped.
fn split_words(name: &str) -> Vec<String> {
    let name = name.rsplit([':', '|']).next().unwrap_or(name);
    let mut words = Vec::new();
    let mut word = String::new();
    let mut prev = ' ';
    for c in name.chars() {
        if !c.is_alphanumeric() {
            if !word.is_empty() {
                words.push(std::mem::take(&mut word));
            }
            prev = ' ';
            continue;
        }
        let case_change = c.is_uppercase() && prev.is_lowercase();
        let digit_change = prev.is_alphanumeric() && c.is_ascii_digit() != prev.is_ascii_digit();
        if (case_change || digit_change) && !word.is_empty() {
            words.push(std::mem::take(&mut word));
        }
        word.extend(c.to_lowercase());
        prev = c;
    }
    if !word.is_empty() {
        words.push(word);
    }
    words
}

#[cfg(test)]
mod tests {
    use super::Class::*;
    use super::Limb::*;
    use super::{Class, classify};

    #[test]
    fn common_rigs() {
        let cases: &[(&str, Option<Class>)] = &[
            // 3ds Max Biped (Metro 2033)
            ("bip01_pelvis", Some(Hips)),
            ("bip01_spine2", Some(Spine)),
            ("bip01_neck", Some(Neck)),
            ("bip01_head", Some(Head)),
            ("bip01_l_clavicle", Some(Limb(0, Shoulder))),
            ("bip01_r_upperarm", Some(Limb(1, UpperArm))),
            ("bip01_l_forearm", Some(Limb(0, LowerArm))),
            ("bip01_r_hand", Some(Limb(1, Hand))),
            ("bip01_l_finger0", Some(Finger(0, 0, false))),
            ("bip01_l_finger02", Some(Finger(0, 0, false))),
            ("bip01_r_finger31", Some(Finger(1, 3, false))),
            ("bip01_l_thigh", Some(Limb(0, UpperLeg))),
            ("bip01_r_calf", Some(Limb(1, LowerLeg))),
            ("bip01_l_foot", Some(Limb(0, Foot))),
            ("bip01_r_toe0", Some(Limb(1, Toes))),
            ("bip01_l_foretwist1", Some(Twist)),
            ("bip01_lcalftwist", Some(Twist)),
            ("bip01", None),
            ("aux_bip01_l_hand", None),
            ("loc_ik_left_hand", None),
            ("bn_jaw", None),
            ("bn_eyeball_l", None),
            // Mixamo
            ("mixamorig:Hips", Some(Hips)),
            ("mixamorig:Spine1", Some(Spine)),
            ("mixamorig:LeftShoulder", Some(Limb(0, Shoulder))),
            ("mixamorig:LeftArm", Some(Limb(0, UpperArm))),
            ("mixamorig:RightForeArm", Some(Limb(1, LowerArm))),
            ("mixamorig:LeftHandIndex2", Some(Finger(0, 1, false))),
            ("mixamorig:RightHandPinky1", Some(Finger(1, 4, false))),
            ("mixamorig:LeftUpLeg", Some(Limb(0, UpperLeg))),
            ("mixamorig:LeftLeg", Some(Limb(0, LowerLeg))),
            ("mixamorig:RightToeBase", Some(Limb(1, Toes))),
            ("mixamorig:HeadTop_End", None),
            // Unreal
            ("pelvis", Some(Hips)),
            ("spine_03", Some(Spine)),
            ("neck_01", Some(Neck)),
            ("clavicle_l", Some(Limb(0, Shoulder))),
            ("upperarm_r", Some(Limb(1, UpperArm))),
            ("lowerarm_l", Some(Limb(0, LowerArm))),
            ("hand_r", Some(Limb(1, Hand))),
            ("thumb_01_l", Some(Finger(0, 0, false))),
            ("index_metacarpal_r", Some(Finger(1, 1, true))),
            ("thigh_l", Some(Limb(0, UpperLeg))),
            ("calf_r", Some(Limb(1, LowerLeg))),
            ("ball_l", Some(Limb(0, Toes))),
            ("lowerarm_twist_01_l", Some(Twist)),
            ("ik_hand_l", None),
            // Unity humanoid / VRM / Character Creator / Rigify
            ("LeftUpperArm", Some(Limb(0, UpperArm))),
            ("RightLowerLeg", Some(Limb(1, LowerLeg))),
            ("UpperChest", Some(Spine)),
            ("LeftLittleDistal", Some(Finger(0, 4, false))),
            ("J_Bip_R_Thumb2", Some(Finger(1, 0, false))),
            ("J_Bip_C_Hips", Some(Hips)),
            ("CC_Base_L_Forearm", Some(Limb(0, LowerArm))),
            ("upper_arm.L", Some(Limb(0, UpperArm))),
            ("shin.R", Some(Limb(1, LowerLeg))),
        ];
        for (name, want) in cases {
            assert_eq!(classify(name), *want, "{name}");
        }
    }
}
