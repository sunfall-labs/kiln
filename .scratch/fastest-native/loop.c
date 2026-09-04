/* What the middle-end should emit for bench/cpu/loop.mjs.
 * Values are NaN-boxed (ADR 0053). The loop is versioned: a guarded unboxed clone runs while
 * `sum` and `i` are numbers; the generic clone is the fallback and the exit boxes the registers. */
#include <stdint.h>
#include <stdio.h>
#include <time.h>
static double now_ms(void){struct timespec t; clock_gettime(CLOCK_MONOTONIC,&t); return t.tv_sec*1e3+t.tv_nsec/1e6;}
#include <math.h>
#include <string.h>
typedef uint64_t kiln_val;
#define IS_NUM(v) ((v) < UINT64_C(0xFFF8000000000000))
static inline double unbox(kiln_val v){ double d; memcpy(&d,&v,8); return d; }
static inline kiln_val box(double d){ kiln_val v; memcpy(&v,&d,8); if ((v & UINT64_C(0x7FF8000000000000)) == UINT64_C(0x7FF8000000000000)) v = UINT64_C(0x7FF8000000000000); return v; }
__attribute__((noinline)) static kiln_val generic_add(kiln_val a, kiln_val b){ return box(unbox(a)+unbox(b)); }
__attribute__((noinline)) static kiln_val generic_mod(kiln_val a, kiln_val b){ return box(fmod(unbox(a),unbox(b))); }
/* `x % c` where both sides are proven integral in [-2^53, 2^53]: the emitter learned that `i`
 * is an induction variable stepping by 1 from 0, so the range is static and the int64 path is
 * emitted with no dynamic integrality test. */
int main(void){ double samples[20]; int rep; for (rep=0; rep<20; rep++){ double t0=now_ms();
  kiln_val locals[2]; locals[0] = box(0.0); locals[1] = box(0.0);
  /* -- specialized clone: entry guard -- */
  if (IS_NUM(locals[0]) && IS_NUM(locals[1])) {
    double sum = unbox(locals[0]); int64_t i = (int64_t)unbox(locals[1]);
    for (; i < 50000000; i++) sum += (double)(i % 7);
    locals[0] = box(sum); locals[1] = box((double)i);
    goto done;
  }
  /* -- generic clone (the code emitted today, minus the stack) -- */
  for (;;) {
    if (!(unbox(locals[1]) < 50000000.0)) break;
    locals[0] = generic_add(locals[0], generic_mod(locals[1], box(7.0)));
    locals[1] = generic_add(locals[1], box(1.0));
  }
done:
  samples[rep]=now_ms()-t0; if (rep==19) printf("%.0f\n", unbox(locals[0])); }
  for (int a=0;a<20;a++) for(int c=a+1;c<20;c++) if (samples[c]<samples[a]){double x=samples[a];samples[a]=samples[c];samples[c]=x;}
  fprintf(stderr, "workload median %.4f ms\n", samples[10]); return 0;
}
