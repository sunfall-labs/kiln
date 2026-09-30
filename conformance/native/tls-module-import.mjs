import tls, { connect, createServer } from 'node:tls';
import bare from 'tls';
console.log(tls === bare, tls.connect === connect, tls.createServer === createServer);
