// Browser render comparison for original and round-tripped pinned glTF assets.
// Requires PLAYWRIGHT_MODULE and THREE_ROOT when packages are not locally installed.
const {chromium}=require(process.env.PLAYWRIGHT_MODULE||'playwright');
const fs=require('node:fs');const path=require('node:path');const http=require('node:http');
const root=path.resolve(__dirname,'../../..');
// Override only the derivative location to compare real preview tiers as well.
const output=path.resolve(root,process.env.CORPUS_EXPORT_DIR||'target/corpus-export');
const suffix=process.env.CORPUS_EXPORT_SUFFIX||'';
const three=process.env.THREE_ROOT||path.dirname(require.resolve('three/package.json'));
const document=`<!doctype html><script type="importmap">{"imports":{"three":"/three/build/three.module.js","three/addons/":"/three/examples/jsm/"}}</script><body><script type="module">
import * as T from 'three';import {GLTFLoader} from 'three/addons/loaders/GLTFLoader.js';
window.compare=async function(id){
 const loader=new GLTFLoader();const a=await loader.loadAsync('/original/'+id);const b=await loader.loadAsync('/export/'+id);
 const renderer=new T.WebGLRenderer({antialias:false,preserveDrawingBuffer:true});renderer.setSize(384,384);renderer.setClearColor(0x202020);document.body.appendChild(renderer.domElement);
 const world=new T.Scene();world.add(new T.HemisphereLight(0xffffff,0x666666,2));
 for(const p of [[1,2,3],[-2,1,-1]]){const light=new T.DirectionalLight(0xffffff,3);light.position.set(...p);world.add(light);}
 const bounds=new T.Box3().setFromObject(a.scene);const center=bounds.getCenter(new T.Vector3());const size=bounds.getSize(new T.Vector3()).length();
 const camera=new T.PerspectiveCamera(35,1,size/1000,size*100);const gl=renderer.getContext();
 const comparisons=[];
 for(const angle of [0.4,1.5,2.8]){
  camera.position.set(center.x+Math.sin(angle)*size*1.5,center.y+size*.6,center.z+Math.cos(angle)*size*1.5);camera.lookAt(center);
  const images=[];
  for(const model of [a,b]){world.add(model.scene);renderer.render(world,camera);const bytes=new Uint8Array(384*384*4);gl.readPixels(0,0,384,384,gl.RGBA,gl.UNSIGNED_BYTE,bytes);images.push(bytes);world.remove(model.scene);}
  let total=0,changed=0,foreground=0;for(let i=0;i<images[0].length;i+=4){if(images[0][i]!==32||images[0][i+1]!==32||images[0][i+2]!==32)foreground++;for(let c=0;c<3;c++){const d=Math.abs(images[0][i+c]-images[1][i+c]);total+=d;if(d>3)changed++;}}
  comparisons.push({mean_absolute_rgb_error:total/(384*384*3),fraction_channels_over_3:changed/(384*384*3),foreground_pixels:foreground});
 }
 renderer.dispose();renderer.domElement.remove();
 for(const model of [a,b])model.scene.traverse(o=>{o.geometry?.dispose();if(o.material)for(const material of Array.isArray(o.material)?o.material:[o.material]){for(const value of Object.values(material))if(value?.isTexture)value.dispose();material.dispose();}});
 return comparisons;
};window.ready=true;
</script>`;
const server=http.createServer((req,res)=>{
 if(req.url==='/'){res.setHeader('Content-Type','text/html');res.end(document);return;}
 let filename;
 if(req.url.startsWith('/three/')){const relative=req.url.slice(7);if(relative.includes('..')){res.writeHead(404).end();return;}filename=path.join(three,relative);}
 else {const match=req.url.match(/^\/(original|export)\/(Box|BoomBox|NormalTangentMirrorTest)$/);if(match){const [,type,id]=match;filename=type==='original'?path.join(root,'target/corpus',id,id+'.glb'):path.join(output,id+suffix+'.glb');}}
 if(!filename||!fs.existsSync(filename)){res.writeHead(404).end();return;}res.setHeader('Content-Type',filename.endsWith('.js')?'text/javascript':'model/gltf-binary');fs.createReadStream(filename).pipe(res);
});
(async()=>{await new Promise(r=>server.listen(0,'127.0.0.1',r));const browser=await chromium.launch({headless:true,executablePath:process.env.CHROMIUM_EXECUTABLE,args:['--enable-unsafe-swiftshader']});try{const page=await browser.newPage();const errors=[];page.on('pageerror',e=>errors.push(e.message));await page.goto('http://127.0.0.1:'+server.address().port);await page.waitForFunction(()=>window.ready);const results=[];for(const id of ['Box','BoomBox','NormalTangentMirrorTest']){const comparisons=await page.evaluate(id=>window.compare(id),id);for(const c of comparisons)if(c.foreground_pixels<1000||c.mean_absolute_rgb_error>0.5||c.fraction_channels_over_3>.01)throw Error(JSON.stringify({id,comparisons}));results.push({id,comparisons});}if(errors.length)throw Error(errors.join('\n'));fs.writeFileSync(path.join(output,'render-report.json'),JSON.stringify(results,null,2));console.log(JSON.stringify(results,null,2));}finally{await browser.close();server.close();}})().catch(e=>{console.error(e);server.close();process.exitCode=1;});
