#ifndef USD_TOOLBOX_H
#define USD_TOOLBOX_H

#pragma once

#include <stdarg.h>
#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>
#include <stdlib.h>

/**
 * C ABI status code.
 */
typedef uint32_t UsdToolboxStatus;

/**
 * Library-owned byte allocation.
 */
typedef struct UsdToolboxBuffer {
  /**
   * First byte, or null for an empty/default buffer.
   */
  uint8_t *data;
  /**
   * Initialized byte count.
   */
  size_t len;
  /**
   * Allocation capacity needed by the matching Rust deallocator.
   */
  size_t capacity;
} UsdToolboxBuffer;

/**
 * C ABI target selector.
 */
typedef uint32_t UsdToolboxTarget;

/**
 * One or more caller output buffers are too small. Required lengths are set.
 */
#define USD_TOOLBOX_STATUS_BUFFER_TOO_SMALL 2

/**
 * Neutral data could not be exported.
 */
#define USD_TOOLBOX_STATUS_EXPORT_ERROR 4

/**
 * Source bytes could not be imported.
 */
#define USD_TOOLBOX_STATUS_IMPORT_ERROR 3

/**
 * Null pointer, invalid UTF-8/JSON, or unknown target.
 */
#define USD_TOOLBOX_STATUS_INVALID_ARGUMENT 1

/**
 * Successful result.
 */
#define USD_TOOLBOX_STATUS_OK 0

/**
 * A panic was caught at the ABI boundary.
 */
#define USD_TOOLBOX_STATUS_PANIC 255

/**
 * Binary GLB target.
 */
#define USD_TOOLBOX_TARGET_GLB 6

/**
 * JSON glTF target.
 */
#define USD_TOOLBOX_TARGET_GLTF 5

/**
 * MaterialX document target.
 */
#define USD_TOOLBOX_TARGET_MATERIALX 4

/**
 * NVIDIA Omniverse-compatible USDZ target.
 */
#define USD_TOOLBOX_TARGET_OMNIVERSE 8

/**
 * Autodesk Revit Generic appearance image-set ZIP target.
 */
#define USD_TOOLBOX_TARGET_REVIT 7

/**
 * USDA text target.
 */
#define USD_TOOLBOX_TARGET_USDA 2

/**
 * USDC crate target.
 */
#define USD_TOOLBOX_TARGET_USDC 3

/**
 * USDZ package target.
 */
#define USD_TOOLBOX_TARGET_USDZ 1

/**
 * Bakes a versioned procedural definition to PBR maps and vector hatches.
 *
 * The JSON result includes each asset's encoded bytes. Filesystem-oriented
 * callers should prefer the `bake-procedural` CLI command, whose report only
 * contains paths and metadata.
 *
 * # Safety
 *
 * Every non-null pointer must be valid for its supplied length/capacity.
 */
UsdToolboxStatus usd_toolbox_bake_procedural(const uint8_t *definition_json,
                                             size_t definition_len,
                                             uint8_t *output_json,
                                             size_t output_capacity,
                                             size_t *required_output);

/**
 * Releases one library-owned buffer and resets the struct to empty.
 *
 * Passing null, or an already-empty buffer, is safe. Passing a copied buffer
 * struct more than once is a caller error and may double-free.
 *
 * # Safety
 *
 * A non-null pointer must refer to the original `UsdToolboxBuffer` returned by this
 * library and must not be freed concurrently or more than once.
 */
void usd_toolbox_buffer_free(struct UsdToolboxBuffer *buffer);

/**
 * Computes a target loss report without writing a target document.
 *
 * # Safety
 *
 * The input pointer must be valid for its length. The output pointer must be
 * valid for its capacity and `required_output` must be writable.
 */
UsdToolboxStatus usd_toolbox_dry_run(const uint8_t *materials_json,
                                     size_t materials_len,
                                     UsdToolboxTarget target,
                                     uint8_t *output_json,
                                     size_t output_capacity,
                                     size_t *required_output);

/**
 * Exports neutral materials using caller-allocated output and loss buffers.
 *
 * `materials_json` must encode an array of neutral `Material` objects.
 * `options_json` may be null/empty to select defaults. `required_output` and
 * `required_losses` must be valid pointers. No partial output is written.
 *
 * # Safety
 *
 * Every non-null pointer must be valid for its accompanying length/capacity,
 * and the two required-length pointers must be writable.
 */
UsdToolboxStatus usd_toolbox_export(const uint8_t *materials_json,
                                    size_t materials_len,
                                    UsdToolboxTarget target,
                                    const uint8_t *options_json,
                                    size_t options_len,
                                    uint8_t *output,
                                    size_t output_capacity,
                                    size_t *required_output,
                                    uint8_t *losses_json,
                                    size_t losses_capacity,
                                    size_t *required_losses);

/**
 * Exports neutral materials into library-owned buffers.
 *
 * Both returned buffers must be released with `usd_toolbox_buffer_free`.
 *
 * # Safety
 *
 * Input pointers must be valid for their lengths. Both output struct pointers
 * must be distinct, valid for writes, and either uninitialized or empty.
 */
UsdToolboxStatus usd_toolbox_export_alloc(const uint8_t *materials_json,
                                          size_t materials_len,
                                          UsdToolboxTarget target,
                                          const uint8_t *options_json,
                                          size_t options_len,
                                          struct UsdToolboxBuffer *output,
                                          struct UsdToolboxBuffer *losses_json);

/**
 * Imports a MaterialX document and writes a neutral-material JSON array.
 *
 * # Safety
 *
 * Input pointers must be valid for their lengths. The output pointer must be
 * valid for its capacity and `required_output` must be writable.
 */
UsdToolboxStatus usd_toolbox_import_materialx(const uint8_t *input,
                                              size_t input_len,
                                              const uint8_t *options_json,
                                              size_t options_len,
                                              uint8_t *output_json,
                                              size_t output_capacity,
                                              size_t *required_output);

/**
 * Imports a JSON array of texture inputs and writes one material plus losses.
 *
 * # Safety
 *
 * Input pointers must be valid for their lengths. The output pointer must be
 * valid for its capacity and `required_output` must be writable.
 */
UsdToolboxStatus usd_toolbox_import_texture_set(const uint8_t *inputs_json,
                                                size_t inputs_len,
                                                const uint8_t *options_json,
                                                size_t options_len,
                                                uint8_t *output_json,
                                                size_t output_capacity,
                                                size_t *required_output);

/**
 * Imports USD/USDZ bytes and writes a neutral-material JSON array.
 *
 * # Safety
 *
 * Input pointers must be valid for their lengths. The output pointer must be
 * valid for its capacity and `required_output` must be writable.
 */
UsdToolboxStatus usd_toolbox_import_usd(const uint8_t *input,
                                        size_t input_len,
                                        const uint8_t *options_json,
                                        size_t options_len,
                                        uint8_t *output_json,
                                        size_t output_capacity,
                                        size_t *required_output);

/**
 * Copies the current thread's last error message using the two-pass buffer contract.
 *
 * # Safety
 *
 * The output pointer must be valid for its capacity and `required_output` must
 * be writable.
 */
UsdToolboxStatus usd_toolbox_last_error(uint8_t *output,
                                        size_t output_capacity,
                                        size_t *required_output);

/**
 * Attaches a generated or manually authored texture using declared lineage.
 *
 * `request_json` is `{ "material": Material, "attachment": TextureAttachment }`.
 *
 * # Safety
 *
 * Every non-null pointer must be valid for its supplied length/capacity.
 */
UsdToolboxStatus usd_toolbox_material_attach_texture(const uint8_t *request_json,
                                                     size_t request_len,
                                                     uint8_t *output_json,
                                                     size_t output_capacity,
                                                     size_t *required_output);

/**
 * Audits neutral materials. Empty `profile_json` selects portable-PBR defaults.
 *
 * # Safety
 *
 * Every non-null pointer must be valid for its supplied length/capacity.
 */
UsdToolboxStatus usd_toolbox_material_audit(const uint8_t *materials_json,
                                            size_t materials_len,
                                            const uint8_t *profile_json,
                                            size_t profile_len,
                                            uint8_t *output_json,
                                            size_t output_capacity,
                                            size_t *required_output);

/**
 * Clusters caller-produced, model-versioned embeddings.
 *
 * # Safety
 *
 * Every non-null pointer must be valid for its supplied length/capacity.
 */
UsdToolboxStatus usd_toolbox_material_cluster(const uint8_t *records_json,
                                              size_t records_len,
                                              const uint8_t *options_json,
                                              size_t options_len,
                                              uint8_t *output_json,
                                              size_t output_capacity,
                                              size_t *required_output);

/**
 * Performs every safe downscale and returns remaining external-generation jobs.
 *
 * # Safety
 *
 * Every non-null pointer must be valid for its supplied length/capacity.
 */
UsdToolboxStatus usd_toolbox_material_complete_tiers(const uint8_t *request_json,
                                                     size_t request_len,
                                                     uint8_t *output_json,
                                                     size_t output_capacity,
                                                     size_t *required_output);

/**
 * Diffs two neutral materials at stable field paths.
 *
 * # Safety
 *
 * Every non-null pointer must be valid for its supplied length/capacity.
 */
UsdToolboxStatus usd_toolbox_material_diff(const uint8_t *before_json,
                                           size_t before_len,
                                           const uint8_t *after_json,
                                           size_t after_len,
                                           uint8_t *output_json,
                                           size_t output_capacity,
                                           size_t *required_output);

/**
 * Builds deterministic input for caller-selected visual/text embedding models.
 *
 * # Safety
 *
 * Every non-null pointer must be valid for its supplied length/capacity.
 */
UsdToolboxStatus usd_toolbox_material_embedding_input(const uint8_t *material_json,
                                                      size_t material_len,
                                                      const uint8_t *options_json,
                                                      size_t options_len,
                                                      uint8_t *output_json,
                                                      size_t output_capacity,
                                                      size_t *required_output);

/**
 * Extracts one exact encoded texture role/tier.
 *
 * `request_json` has the same `material`, `role`, and `tier` fields as remove.
 *
 * # Safety
 *
 * Every non-null pointer must be valid for its supplied length/capacity.
 */
UsdToolboxStatus usd_toolbox_material_extract_texture(const uint8_t *request_json,
                                                      size_t request_len,
                                                      uint8_t *output,
                                                      size_t output_capacity,
                                                      size_t *required_output);

/**
 * Inspects neutral materials and writes compact JSON without texture bytes.
 *
 * # Safety
 *
 * Input/output pointers follow the same two-pass rules as `usd_toolbox_dry_run`.
 */
UsdToolboxStatus usd_toolbox_material_inspect(const uint8_t *materials_json,
                                              size_t materials_len,
                                              uint8_t *output_json,
                                              size_t output_capacity,
                                              size_t *required_output);

/**
 * Migrates legacy neutral JSON into the current versioned document envelope.
 *
 * # Safety
 *
 * Every non-null pointer must be valid for its supplied length/capacity.
 */
UsdToolboxStatus usd_toolbox_material_migrate(const uint8_t *input_json,
                                              size_t input_len,
                                              uint8_t *output_json,
                                              size_t output_capacity,
                                              size_t *required_output);

/**
 * Applies a validated JSON merge patch to one neutral material.
 *
 * # Safety
 *
 * Every non-null pointer must be valid for its supplied length/capacity.
 */
UsdToolboxStatus usd_toolbox_material_patch(const uint8_t *material_json,
                                            size_t material_len,
                                            const uint8_t *patch_json,
                                            size_t patch_len,
                                            uint8_t *output_json,
                                            size_t output_capacity,
                                            size_t *required_output);

/**
 * Plans missing texture-tier work without invoking an external generator.
 *
 * # Safety
 *
 * Every non-null pointer must be valid for its supplied length/capacity.
 */
UsdToolboxStatus usd_toolbox_material_plan_tiers(const uint8_t *material_json,
                                                 size_t material_len,
                                                 const uint8_t *policy_json,
                                                 size_t policy_len,
                                                 uint8_t *output_json,
                                                 size_t output_capacity,
                                                 size_t *required_output);

/**
 * Removes one exact texture role/tier from a neutral material.
 *
 * # Safety
 *
 * Every non-null pointer must be valid for its supplied length/capacity.
 */
UsdToolboxStatus usd_toolbox_material_remove_texture(const uint8_t *request_json,
                                                     size_t request_len,
                                                     uint8_t *output_json,
                                                     size_t output_capacity,
                                                     size_t *required_output);

/**
 * Returns the static library semantic version bytes. The pointer must not be freed.
 *
 * # Safety
 *
 * A non-null `length` pointer must be valid for a `usize` write.
 */
const uint8_t *usd_toolbox_version(size_t *length);

#endif  /* USD_TOOLBOX_H */
