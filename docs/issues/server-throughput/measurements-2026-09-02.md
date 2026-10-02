# Server throughput, 2026-09-02

`bench/server/compare.sh` at 674a8280 plus the compare.sh fixes; four client processes, sixteen connections each, five seconds, three interleaved passes, one machine (load average 6-7 from other agents' builds throughout). Median rps across passes; rss is the server's peak resident set after a run.

```
hello  kiln  rps med   163709 (min 159888, max 164260)  p50 0.324  p99max 1.02  err 0  rss 8 MiB
hello  bun   rps med   142532 (min 137992, max 144005)  p50 0.36  p99max 1.212  err 0  rss 76 MiB
hello  node  rps med   115047 (min 114012, max 115175)  p50 0.478  p99max 1.392  err 0  rss 84 MiB
hello  deno  rps med   104061 (min 101888, max 104499)  p50 0.541  p99max 1.369  err 0  rss 121 MiB
mixed  kiln  rps med   111404 (min 111226, max 113301)  p50 0.439  p99max 1.683  err 0  rss 10 MiB
mixed  bun   rps med   107572 (min 105965, max 108386)  p50 0.418  p99max 2.042  err 0  rss 94 MiB
mixed  node  rps med    89035 (min 88419, max 89272)  p50 0.569  p99max 2.004  err 0  rss 101 MiB
mixed  deno  rps med    78767 (min 77444, max 78890)  p50 0.62  p99max 2.891  err 0  rss 126 MiB
```

Raw log:

```
pass=1 workload=hello runtime=kiln {"processes": 4, "connections": 64, "rps": 164260, "errors": 0, "wrong": 0, "p50_ms": 0.322, "p99_max_ms": 1.011}
  rss_kib=8336
pass=1 workload=hello runtime=node {"processes": 4, "connections": 64, "rps": 115175, "errors": 0, "wrong": 0, "p50_ms": 0.473, "p99_max_ms": 1.469}
  rss_kib=85856
pass=1 workload=hello runtime=bun  {"processes": 4, "connections": 64, "rps": 144005, "errors": 0, "wrong": 0, "p50_ms": 0.354, "p99_max_ms": 1.212}
  rss_kib=77376
pass=1 workload=hello runtime=deno {"processes": 4, "connections": 64, "rps": 101888, "errors": 0, "wrong": 0, "p50_ms": 0.547, "p99_max_ms": 1.453}
  rss_kib=120160
pass=1 workload=mixed runtime=kiln {"processes": 4, "connections": 64, "rps": 111404, "errors": 0, "wrong": 0, "p50_ms": 0.439, "p99_max_ms": 1.683}
  rss_kib=10176
pass=1 workload=mixed runtime=node {"processes": 4, "connections": 64, "rps": 88419, "errors": 0, "wrong": 0, "p50_ms": 0.572, "p99_max_ms": 2.036}
  rss_kib=98688
pass=1 workload=mixed runtime=bun  {"processes": 4, "connections": 64, "rps": 105965, "errors": 0, "wrong": 0, "p50_ms": 0.422, "p99_max_ms": 2.091}
  rss_kib=93216
pass=1 workload=mixed runtime=deno {"processes": 4, "connections": 64, "rps": 77444, "errors": 0, "wrong": 0, "p50_ms": 0.629, "p99_max_ms": 2.921}
  rss_kib=129328
pass=2 workload=hello runtime=kiln {"processes": 4, "connections": 64, "rps": 159888, "errors": 0, "wrong": 0, "p50_ms": 0.331, "p99_max_ms": 1.037}
  rss_kib=8496
pass=2 workload=hello runtime=node {"processes": 4, "connections": 64, "rps": 114012, "errors": 0, "wrong": 0, "p50_ms": 0.481, "p99_max_ms": 1.413}
  rss_kib=86080
pass=2 workload=hello runtime=bun  {"processes": 4, "connections": 64, "rps": 137992, "errors": 0, "wrong": 0, "p50_ms": 0.366, "p99_max_ms": 1.263}
  rss_kib=77696
pass=2 workload=hello runtime=deno {"processes": 4, "connections": 64, "rps": 104499, "errors": 0, "wrong": 0, "p50_ms": 0.539, "p99_max_ms": 1.367}
  rss_kib=123936
pass=2 workload=mixed runtime=kiln {"processes": 4, "connections": 64, "rps": 111226, "errors": 0, "wrong": 0, "p50_ms": 0.44, "p99_max_ms": 1.693}
  rss_kib=10432
pass=2 workload=mixed runtime=node {"processes": 4, "connections": 64, "rps": 89272, "errors": 0, "wrong": 0, "p50_ms": 0.573, "p99_max_ms": 1.975}
  rss_kib=101728
pass=2 workload=mixed runtime=bun  {"processes": 4, "connections": 64, "rps": 108386, "errors": 0, "wrong": 0, "p50_ms": 0.411, "p99_max_ms": 2.043}
  rss_kib=93056
pass=2 workload=mixed runtime=deno {"processes": 4, "connections": 64, "rps": 78767, "errors": 0, "wrong": 0, "p50_ms": 0.62, "p99_max_ms": 2.891}
  rss_kib=127968
pass=3 workload=hello runtime=kiln {"processes": 4, "connections": 64, "rps": 163709, "errors": 0, "wrong": 0, "p50_ms": 0.324, "p99_max_ms": 1.02}
  rss_kib=8512
pass=3 workload=hello runtime=node {"processes": 4, "connections": 64, "rps": 115047, "errors": 0, "wrong": 0, "p50_ms": 0.478, "p99_max_ms": 1.392}
  rss_kib=85712
pass=3 workload=hello runtime=bun  {"processes": 4, "connections": 64, "rps": 142532, "errors": 0, "wrong": 0, "p50_ms": 0.36, "p99_max_ms": 1.212}
  rss_kib=78400
pass=3 workload=hello runtime=deno {"processes": 4, "connections": 64, "rps": 104061, "errors": 0, "wrong": 0, "p50_ms": 0.541, "p99_max_ms": 1.369}
  rss_kib=120640
pass=3 workload=mixed runtime=kiln {"processes": 4, "connections": 64, "rps": 113301, "errors": 0, "wrong": 0, "p50_ms": 0.433, "p99_max_ms": 1.648}
  rss_kib=10480
pass=3 workload=mixed runtime=node {"processes": 4, "connections": 64, "rps": 89035, "errors": 0, "wrong": 0, "p50_ms": 0.569, "p99_max_ms": 2.004}
  rss_kib=104016
pass=3 workload=mixed runtime=bun  {"processes": 4, "connections": 64, "rps": 107572, "errors": 0, "wrong": 0, "p50_ms": 0.418, "p99_max_ms": 2.042}
  rss_kib=97008
pass=3 workload=mixed runtime=deno {"processes": 4, "connections": 64, "rps": 78890, "errors": 0, "wrong": 0, "p50_ms": 0.621, "p99_max_ms": 2.906}
  rss_kib=128960
```
