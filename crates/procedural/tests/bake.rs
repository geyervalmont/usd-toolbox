use image::GenericImageView;
use usd_toolbox_procedural::{Bond, Colour, Masonry, Paint, ProceduralDefinition, Recipe, bake};

fn paint() -> ProceduralDefinition {
    ProceduralDefinition {
        schema: 1,
        width_px: 64,
        height_px: 32,
        width_mm: 1000.0,
        height_mm: 500.0,
        seed: 42,
        recipe: Recipe::Paint(Paint {
            colour: Colour::new(211, 204, 189),
            roughness: 0.62,
            variation: 0.03,
            texture_depth: 0.1,
        }),
    }
}

#[test]
fn paint_bakes_a_complete_deterministic_pbr_set() {
    let first = bake(&paint()).unwrap();
    let second = bake(&paint()).unwrap();

    assert_eq!(first, second);
    assert_eq!(
        first.assets.iter().map(|asset| asset.role.as_str()).collect::<Vec<_>>(),
        ["base_color", "normal", "roughness", "height", "metallic",]
    );
    let base = &first.assets[0];
    assert_eq!(image::load_from_memory(&base.bytes).unwrap().dimensions(), (64, 32));
    assert_eq!(first.definition_digest.len(), 64);
}

#[test]
fn masonry_adds_editable_vector_and_revit_hatches() {
    let definition = ProceduralDefinition {
        schema: 1,
        width_px: 64,
        height_px: 64,
        width_mm: 480.0,
        height_mm: 172.0,
        seed: 7,
        recipe: Recipe::Masonry(Masonry {
            unit_width_mm: 230.0,
            unit_height_mm: 76.0,
            joint_mm: 10.0,
            bond: Bond::Running,
            unit_colours: vec![Colour::new(160, 80, 55)],
            joint_colour: Colour::new(205, 202, 192),
            roughness: 0.68,
            edge_depth_mm: 3.0,
            tone_variation: 0.12,
        }),
    };
    let baked = bake(&definition).unwrap();
    let pat = baked.assets.iter().find(|asset| asset.role == "hatch_pat").unwrap();
    let svg = baked.assets.iter().find(|asset| asset.role == "hatch_svg").unwrap();

    assert!(String::from_utf8_lossy(&pat.bytes).contains(";%TYPE=MODEL"));
    assert!(String::from_utf8_lossy(&svg.bytes).contains("<svg"));
    assert!(baked.assets.iter().all(|asset| asset.sha256.len() == 64));
}

#[test]
fn malformed_and_unbounded_recipes_are_refused() {
    let mut definition = paint();
    definition.width_px = 9000;
    assert!(
        bake(&definition)
            .unwrap_err()
            .to_string()
            .contains("between 1 and 8192")
    );

    let json = r##"{"schema":1,"generator":"paint","parameters":{"colour":"#ffffff","mystery":2}}"##;
    assert!(serde_json::from_str::<ProceduralDefinition>(json).is_err());
}
