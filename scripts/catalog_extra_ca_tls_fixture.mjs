// Temporary HTTPS fixture and unchanged-source client; no external route.
import * as fs from 'node:fs';
import { pathToFileURL } from 'node:url';
const [mode,certPath,keyPath,readyPath]=process.argv.slice(2);
if(mode==='--server') {
 const server=Bun.serve({hostname:'127.0.0.1',port:0,tls:{cert:fs.readFileSync(certPath),key:fs.readFileSync(keyPath)},fetch:()=>Response.json({models:[]})});
 fs.writeFileSync(readyPath,JSON.stringify({port:server.port}));
} else if(mode==='--original') {
 // argv: mode, source-file, base-url, cert-file
 const source=certPath,url=keyPath,pem=readyPath;
 const [replacementPem,replacementUrl]=process.argv.slice(6);
 delete process.env.NODE_EXTRA_CA_CERTS;
 const module=await import(pathToFileURL(source).href);
 let untrusted=false;
 try{await fetch(url);}catch{untrusted=true;}
 process.env.NODE_EXTRA_CA_CERTS=pem;
 const wrapped=module.wrapFetchForExtraCa(fetch);
 const response=await wrapped(url);
 const payload=await response.json();
 const originalPem=fs.readFileSync(pem);
 await Bun.sleep(5);
 fs.writeFileSync(pem,fs.readFileSync(replacementPem));
 let rotationOldRejected=false;
 try{await wrapped(url);}catch{rotationOldRejected=true;}
 const replacementResponse=await wrapped(replacementUrl);
 const rotationPayload=await replacementResponse.json();
 await Bun.sleep(5);
 fs.writeFileSync(pem,originalPem);
 process.env.NODE_EXTRA_CA_CERTS=pem+'-missing';
 let missingName;
 try{await wrapped(url);}catch(error){missingName=error.name;}
 console.log(JSON.stringify({untrusted,trustedStatus:response.status,payload,rotationOldRejected,rotationNewStatus:replacementResponse.status,rotationPayload,missingName}));
} else throw Error('unknown mode');
