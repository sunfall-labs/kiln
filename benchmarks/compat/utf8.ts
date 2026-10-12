const bytes=new TextEncoder().encode('café'); console.log(JSON.stringify([Array.from(bytes),new TextDecoder().decode(bytes)]));
