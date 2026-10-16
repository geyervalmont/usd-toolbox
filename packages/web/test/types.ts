import { Toolbox, type MetadataProperty, type QueryRequest } from '../index.js';
const request: QueryRequest = { fields: ['name', 'bounds'], filter: { metadata: [{ key: 'product_id', operator: 'exists', exists: true }] } };
async function example(bytes: Uint8Array) {
  const toolbox = new Toolbox();
  try {
    const model = await toolbox.open(bytes, { document: { metadata_indexes: [{ key: 'product_id' }] } });
    const page = await model.query(request);
    const id = page.rows[0].id;
    const property: MetadataProperty | null = await model.getProperty({ scope: 'node', id }, 'product_id');
    console.log(property, await model.count(), await model.ancestors(id), page.rows[0].fields.bounds?.value?.min);
    await model.dispose();
  } finally { toolbox.dispose(); }
}
void example;
