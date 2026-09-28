const path = require('node:path');
const Piscina = require('piscina');

(async () => {
  const pool = new Piscina({
    filename: path.join(__dirname, 'piscina-task.cjs'),
    minThreads: 1,
    maxThreads: 1,
  });
  console.log(await pool.run(41));
  await pool.destroy();
})();
