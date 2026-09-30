// kiln eval smoke test (authored for kiln, not from porffor).
const express = require('express');
const app = express();
console.log(typeof app, typeof app.get, typeof app.listen);
app.set('title', 'demo');
console.log(app.get('title'), app.get('env'));
const router = express.Router();
router.get('/x', (req, res) => res.send('x'));
console.log(typeof router);
console.log(typeof express.json(), typeof express.static('public'));
const app2 = express();
app2.route('/things').get((req, res) => res.send('list')).post((req, res) => res.send('create'));
console.log('routes-ok');
