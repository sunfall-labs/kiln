import { randomUUID } from 'node:crypto';
import { URL } from 'node:url';

const uuid = randomUUID();
if (!/^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/.test(uuid)) {
  throw new Error('uuid shape');
}
const url = new URL('/health?q=1', 'http://localhost:3000');
if (url.pathname !== '/health' || url.searchParams.get('q') !== '1') throw new Error('URL');
// A UUID differs on every run (it is drawn from the OS CSPRNG), so only its shape is printed.
console.log('uuid v4 ok', randomUUID() !== uuid);
