// kiln eval smoke test (authored for kiln, not from porffor).
const { QueryClient, QueryCache } = require('@tanstack/query-core');

(async () => {
  const client = new QueryClient();
  const data = await client.fetchQuery({
    queryKey: ['user', 1],
    queryFn: async () => ({ id: 1, name: 'kiln' })
  });
  console.log('fetchQuery:', JSON.stringify(data));

  client.setQueryData(['user', 2], { id: 2, name: 'set' });
  console.log('setQueryData:', JSON.stringify(client.getQueryData(['user', 2])));

  const cached = client.getQueryCache().find({ queryKey: ['user', 1] });
  console.log('cache hit:', cached && cached.state.data.id === 1, 'status:', cached.state.status);

  await client.invalidateQueries({ queryKey: ['user'] });
  const after = client.getQueryCache().find({ queryKey: ['user', 1] });
  console.log('after invalidate isInvalidated:', after.state.isInvalidated);

  const def = client.getDefaultOptions();
  console.log('defaultOptions is object:', typeof def === 'object');

  const cache = new QueryCache();
  const client2 = new QueryClient({ queryCache: cache });
  console.log('custom cache:', client2.getQueryCache() === cache);

  client.clear();
  console.log('cleared:', client.getQueryCache().getAll().length === 0);
})().catch((e) => { console.error('FAILED', e); process.exit(1); });
