//! Checks against the local asset cache (`metro-extract`). Run with
//! `cargo test -p rustvr-guns --test local_assets -- --ignored` on a machine that has it.

/// The 2012's laser shines from its own mount for the laser attachment (`loc_lazer`), on the gun
/// ahead of the grip and below the line of sight, not from the muzzle or the eyes.
#[test]
#[ignore = "needs the local asset cache"]
fn the_2012s_laser_has_its_mount() {
    let d = rustvr_guns::metro::metro_gun_desc(&rustvr_asset::default_root().join("metro2033"), "wpn_2012").unwrap();
    let (laser, grip, sight, muzzle) = (glam::Vec3::from_array(d.laser), glam::Vec3::from_array(d.grip), glam::Vec3::from_array(d.sight), glam::Vec3::from_array(d.muzzle));
    eprintln!("laser {laser:?} grip {grip:?} sight {sight:?} muzzle {muzzle:?}");
    assert!(laser != glam::Vec3::ZERO);
    assert!(laser.z > grip.z && laser.z < muzzle.z + 0.05, "along the gun");
    assert!(laser.distance(sight) < 0.4);
}
