import {parentPort,workerData} from 'node:worker_threads';
globalThis.self=globalThis;
globalThis.postMessage=value=>parentPort.postMessage(value);
await import(workerData.module);
parentPort.on('message',data=>globalThis.onmessage({data}));
parentPort.postMessage({ready:true});
