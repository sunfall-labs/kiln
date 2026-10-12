import { Buffer } from 'node:buffer'; console.log(JSON.stringify([Buffer.from('café').toString('hex'),Buffer.from('YQ==','base64').toString()]));
