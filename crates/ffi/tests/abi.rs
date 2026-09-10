//! C ABI smoke tests exercise the two-pass and ownership contracts.

use usd_toolbox_core::Material;
use usd_toolbox_ffi::*;

#[test]
fn caller_allocated_export_uses_two_pass_contract() {
    let input = serde_json::to_vec(&vec![Material::new("plain", "Plain")]).unwrap();
    let mut output_len = 0;
    let mut losses_len = 0;
    let status = unsafe {
        usd_toolbox_export(
            input.as_ptr(),
            input.len(),
            USD_TOOLBOX_TARGET_USDZ,
            std::ptr::null(),
            0,
            std::ptr::null_mut(),
            0,
            &mut output_len,
            std::ptr::null_mut(),
            0,
            &mut losses_len,
        )
    };
    assert_eq!(status, USD_TOOLBOX_STATUS_BUFFER_TOO_SMALL);
    assert!(output_len > 0);
    assert!(losses_len > 0);

    let mut output = vec![0; output_len];
    let mut losses = vec![0; losses_len];
    let status = unsafe {
        usd_toolbox_export(
            input.as_ptr(),
            input.len(),
            USD_TOOLBOX_TARGET_USDZ,
            std::ptr::null(),
            0,
            output.as_mut_ptr(),
            output.len(),
            &mut output_len,
            losses.as_mut_ptr(),
            losses.len(),
            &mut losses_len,
        )
    };
    assert_eq!(status, USD_TOOLBOX_STATUS_OK);
    assert!(output.starts_with(b"PK"));
    assert_eq!(losses, b"[]");
}

#[test]
fn allocated_buffers_are_explicitly_freed() {
    let input = serde_json::to_vec(&vec![Material::new("plain", "Plain")]).unwrap();
    let mut output = UsdToolboxBuffer::default();
    let mut losses = UsdToolboxBuffer::default();
    let status = unsafe {
        usd_toolbox_export_alloc(
            input.as_ptr(),
            input.len(),
            USD_TOOLBOX_TARGET_GLB,
            std::ptr::null(),
            0,
            &mut output,
            &mut losses,
        )
    };
    assert_eq!(status, USD_TOOLBOX_STATUS_OK);
    assert!(!output.data.is_null());
    unsafe {
        usd_toolbox_buffer_free(&mut output);
        usd_toolbox_buffer_free(&mut losses);
    }
    assert!(output.data.is_null());
    assert!(losses.data.is_null());
}

#[test]
fn material_inspection_uses_the_same_two_pass_contract() {
    let input = serde_json::to_vec(&vec![Material::new("plain", "Plain")]).unwrap();
    let mut required = 0;
    let status =
        unsafe { usd_toolbox_material_inspect(input.as_ptr(), input.len(), std::ptr::null_mut(), 0, &mut required) };
    assert_eq!(status, USD_TOOLBOX_STATUS_BUFFER_TOO_SMALL);
    let mut output = vec![0; required];
    let status = unsafe {
        usd_toolbox_material_inspect(
            input.as_ptr(),
            input.len(),
            output.as_mut_ptr(),
            output.len(),
            &mut required,
        )
    };
    assert_eq!(status, USD_TOOLBOX_STATUS_OK);
    let inspection: serde_json::Value = serde_json::from_slice(&output).unwrap();
    assert_eq!(inspection["schema"], "usd-toolbox.inspection.v1");
    assert_eq!(inspection["material_count"], 1);
}

#[test]
fn procedural_bake_is_available_through_the_two_pass_contract() {
    let definition = br##"{"schema":1,"width_px":8,"height_px":8,"width_mm":100,"height_mm":100,"seed":1,"generator":"paint","parameters":{"colour":"#e1ded4","roughness":0.6,"variation":0.01,"texture_depth":0.1}}"##;
    let mut required = 0;
    let status = unsafe {
        usd_toolbox_bake_procedural(
            definition.as_ptr(),
            definition.len(),
            std::ptr::null_mut(),
            0,
            &mut required,
        )
    };
    assert_eq!(status, USD_TOOLBOX_STATUS_BUFFER_TOO_SMALL);
    let mut output = vec![0; required];
    let status = unsafe {
        usd_toolbox_bake_procedural(
            definition.as_ptr(),
            definition.len(),
            output.as_mut_ptr(),
            output.len(),
            &mut required,
        )
    };
    assert_eq!(status, USD_TOOLBOX_STATUS_OK);
    let result: serde_json::Value = serde_json::from_slice(&output).unwrap();
    assert_eq!(result["generator"], "paint");
    assert_eq!(result["assets"].as_array().unwrap().len(), 5);
}
