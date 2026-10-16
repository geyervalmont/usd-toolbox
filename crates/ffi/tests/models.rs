use std::ptr::{null, null_mut};
use usd_toolbox_ffi::*;

const SOURCE: &[u8] = br#"#usda 1.0
def Mesh "Triangle" {
    uniform token subdivisionScheme = "none"
    point3f[] points = [(0,0,0),(1,0,0),(0,1,0)]
    int[] faceVertexCounts = [3]
    int[] faceVertexIndices = [0,1,2]
}
"#;

#[test]
fn model_abi_import_export_operations_and_short_buffers() {
    let (mut scene_len, mut report_len) = (0, 0);
    let status = unsafe {
        usd_toolbox_model_import_v1(
            SOURCE.as_ptr(),
            SOURCE.len(),
            null(),
            0,
            null_mut(),
            0,
            &mut scene_len,
            null_mut(),
            0,
            &mut report_len,
        )
    };
    assert_eq!(status, USD_TOOLBOX_STATUS_BUFFER_TOO_SMALL);
    let (mut scene, mut report) = (vec![0; scene_len], vec![0; report_len]);
    let status = unsafe {
        usd_toolbox_model_import_v1(
            SOURCE.as_ptr(),
            SOURCE.len(),
            null(),
            0,
            scene.as_mut_ptr(),
            scene.len(),
            &mut scene_len,
            report.as_mut_ptr(),
            report.len(),
            &mut report_len,
        )
    };
    assert_eq!(status, USD_TOOLBOX_STATUS_OK);
    let document: serde_json::Value = serde_json::from_slice(&scene).unwrap();
    assert_eq!(document["schema"], "usd-toolbox.scene.v1");
    let (mut output_len, mut losses_len) = (0, 0);
    assert_eq!(
        unsafe {
            usd_toolbox_model_export_v1(
                scene.as_ptr(),
                scene.len(),
                USD_TOOLBOX_TARGET_GLB,
                null_mut(),
                0,
                &mut output_len,
                null_mut(),
                0,
                &mut losses_len,
            )
        },
        USD_TOOLBOX_STATUS_BUFFER_TOO_SMALL
    );
    let (mut output, mut losses) = (vec![0; output_len], vec![0; losses_len]);
    assert_eq!(
        unsafe {
            usd_toolbox_model_export_v1(
                scene.as_ptr(),
                scene.len(),
                USD_TOOLBOX_TARGET_GLB,
                output.as_mut_ptr(),
                output.len(),
                &mut output_len,
                losses.as_mut_ptr(),
                losses.len(),
                &mut losses_len,
            )
        },
        USD_TOOLBOX_STATUS_OK
    );
    assert!(output.starts_with(b"glTF"));
    let request = serde_json::to_vec(&serde_json::json!({"operation":"inspect","scene":document})).unwrap();
    let mut length = 0;
    assert_eq!(
        unsafe { usd_toolbox_model_operation_v1(request.as_ptr(), request.len(), null_mut(), 0, &mut length) },
        USD_TOOLBOX_STATUS_BUFFER_TOO_SMALL
    );
    let mut bytes = vec![0; length];
    assert_eq!(
        unsafe {
            usd_toolbox_model_operation_v1(
                request.as_ptr(),
                request.len(),
                bytes.as_mut_ptr(),
                bytes.len(),
                &mut length,
            )
        },
        USD_TOOLBOX_STATUS_OK
    );
    let inspection: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(inspection["unique_faces"], 1);
    assert_eq!(
        unsafe { usd_toolbox_model_operation_v1(null(), 5, null_mut(), 0, &mut length) },
        USD_TOOLBOX_STATUS_INVALID_ARGUMENT
    );
    let bad = b"{}";
    assert_ne!(
        unsafe { usd_toolbox_model_operation_v1(bad.as_ptr(), bad.len(), null_mut(), 0, &mut length) },
        USD_TOOLBOX_STATUS_OK
    );
}
