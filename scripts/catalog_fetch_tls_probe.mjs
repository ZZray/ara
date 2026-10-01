// Real Bun fetch TLS option observations; one process per client scenario.
import * as fs from 'node:fs';
import * as https from 'node:https';
import {pathToFileURL} from 'node:url';
import {createHash} from 'node:crypto';
const [mode,file]=process.argv.slice(2),config=JSON.parse(fs.readFileSync(file,'utf8'));
if(Bun.version!=='1.4.0'||createHash('sha256').update(fs.readFileSync(process.execPath)).digest('hex')!=='627d2e4775c24bdedee2cd7ccc18dcadae061e5345274ab6e3c4c797927bfb8f')throw Error('wrong Bun');
if(mode==='--http-probe'){
 const requests=[],cases=[];
 const server=Bun.serve({hostname:'127.0.0.1',port:0,fetch(req){requests.push(new URL(req.url).pathname);return new Response('cleartext accepted');}});
 for(const[id,tls]of Object.entries({invalidCa:{ca:'garbage'},invalidCipher:{ciphers:'NOT-A-CIPHER'},certOnly:{cert:'garbage'},invalidName:{serverName:'bad name'}})){
  try{const response=await fetch(new URL('/'+id,server.url),{tls,signal:AbortSignal.timeout(1200)});cases.push({id,tls,ok:true,status:response.status,body:await response.text()});}
  catch(error){cases.push({id,tls,ok:false,name:error.name,message:error.message});}
 }
 server.stop(true);console.log(JSON.stringify({bunVersion:Bun.version,bunSha256:createHash('sha256').update(fs.readFileSync(process.execPath)).digest('hex'),cases,requests}));
}else if(mode==='--server'){
 const receipts=[],ports={};
 for(const[name,item]of Object.entries(config.servers)){
  const server=https.createServer({cert:fs.readFileSync(item.cert),key:fs.readFileSync(item.key),...(item.ca?{ca:fs.readFileSync(item.ca),requestCert:true,rejectUnauthorized:true}:{}),...(item.ciphers?{ciphers:item.ciphers,minVersion:'TLSv1.2',maxVersion:'TLSv1.2'}:{}),...(item.minVersion?{minVersion:item.minVersion,maxVersion:item.maxVersion}:{})},(req,res)=>{const receipt={server:name,path:req.url,servername:req.socket.servername??null,authorized:req.socket.authorized,cipher:req.socket.getCipher?.()??null,peerCN:req.socket.getPeerCertificate?.()?.subject?.CN??null};receipts.push(receipt);fs.writeFileSync(config.receipts,JSON.stringify(receipts));res.setHeader('Connection','close');res.end(JSON.stringify(receipt));});
  server.on('tlsClientError',()=>{});await new Promise(resolve=>server.listen(0,'127.0.0.1',resolve));ports[name]=server.address().port;
 }
 fs.writeFileSync(config.ready,JSON.stringify(ports));
}else{
 const tls={...config.tls};for(const key of ['ca','cert','key'])if(tls[key])tls[key]=Array.isArray(tls[key])?tls[key].map(p=>fs.readFileSync(p,'utf8')):fs.readFileSync(tls[key],'utf8');
 let fetchImpl=fetch;
 if(config.wrapper){const source=await import(pathToFileURL(config.source).href);fetchImpl=source.wrapFetchForExtraCa(fetch);}
 let result;try{const response=await fetchImpl(config.url,{tls,signal:AbortSignal.timeout(1200),headers:{Connection:'close'}});result={ok:true,status:response.status,body:await response.json()};}catch(error){result={ok:false,name:error.name,code:error.code??null,message:error.message};}
 console.log(JSON.stringify({id:config.id,startupEnv:process.env.NODE_EXTRA_CA_CERTS??null,tlsKeys:Object.keys(tls),...result}));
}
