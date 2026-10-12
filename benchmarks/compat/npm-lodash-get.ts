import get from 'lodash.get'; console.log(JSON.stringify([get({a:{b:42}},'a.b'),get({},'missing','fallback')]));
