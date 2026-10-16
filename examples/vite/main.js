// Copy the generated package into public/toolbox before `vite build`. Keeping the
// worker and WASM together under a static public URL avoids bundler URL rewriting.
const base = new URL(`${import.meta.env.BASE_URL}toolbox/`, location.href);
const { Toolbox } = await import(/* @vite-ignore */ new URL('index.js', base).href);
const toolbox = new Toolbox();
await toolbox.ready();
window.toolboxExample = toolbox;
document.querySelector('#file').addEventListener('change', async (event) => {
  const file = event.target.files[0];
  if (!file) return;
  let model;
  try {
    model = await toolbox.open(await file.arrayBuffer(), { name: file.name, transfer: true });
    const rows = await model.query({ fields: ['name', 'bounds'], limit: 50 });
    document.querySelector('#result').textContent = JSON.stringify(rows, null, 2);
  } catch (error) { document.querySelector('#result').textContent = error.message; }
  finally { await model?.dispose(); }
});
window.addEventListener('pagehide', () => toolbox.dispose(), { once: true });
