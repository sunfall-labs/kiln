const e=new Error('outer',{cause:new Error('inner')}); console.log(JSON.stringify([e.message,e.cause.message]));
