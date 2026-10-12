const b=new ArrayBuffer(8); const v=new DataView(b); v.setUint32(0,42,true); console.log(JSON.stringify([v.getUint32(0,true),Array.from(new Uint8Array(b))]));
