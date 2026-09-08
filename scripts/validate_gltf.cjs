#!/usr/bin/env node

const fs = require("node:fs");
const validator = require("gltf-validator");

if (process.argv.length !== 3) {
  console.error(`usage: ${process.argv[1]} path/to/file.glb`);
  process.exit(2);
}

const path = process.argv[2];
const bytes = new Uint8Array(fs.readFileSync(path));

validator
  .validateBytes(bytes, { uri: path })
  .then((report) => {
    for (const message of report.issues.messages) {
      console.error(`${message.severity}: ${message.code}: ${message.message}`);
    }
    if (report.issues.numErrors > 0 || report.issues.numWarnings > 0) {
      process.exit(1);
    }
    console.log(`Khronos glTF Validator accepted ${path}`);
  })
  .catch((error) => {
    console.error(error);
    process.exit(1);
  });
