# Fixtures

`golden/` contains small, readable outputs from the public reference-material
examples. Tests compare these byte-for-byte to catch serializer changes, while CI
also feeds freshly generated documents to the external OpenUSD, MaterialX, and
glTF validators.

Large supplier corpora and fuzz findings should not be committed here unless
their redistribution rights and provenance are recorded.
