/* What the middle-end should emit for bench/cpu/fib.mjs: a typed entry `fib_num(double)`
 * beside the generic kiln_body_fn entry. The generic entry guards, unboxes, and calls the typed
 * one; the recursive site inside the typed body is a direct C call because `fib` is a module-
 * scope function declaration that is never reassigned. */
#include <stdint.h>
#include <stdio.h>
#include <time.h>
static double now_ms(void){struct timespec t; clock_gettime(CLOCK_MONOTONIC,&t); return t.tv_sec*1e3+t.tv_nsec/1e6;}
#include <string.h>
typedef uint64_t kiln_val;
#define IS_NUM(v) ((v) < UINT64_C(0xFFF8000000000000))
static inline double unbox(kiln_val v){ double d; memcpy(&d,&v,8); return d; }
static inline kiln_val box(double d){ kiln_val v; memcpy(&v,&d,8); return v; }
static double fib_num(double n){ return n < 2 ? n : fib_num(n-1) + fib_num(n-2); }
/* generic entry: what a dynamic caller reaches */
__attribute__((noinline)) static kiln_val fib_generic(kiln_val n){
  if (IS_NUM(n)) return box(fib_num(unbox(n)));
  return UINT64_C(0xFFF8000000000001); /* would be the generic body */
}
int main(void){ double samples[20]; int rep; for (rep=0; rep<20; rep++){ double t0=now_ms();
  /* `volatile`, or clang hoists the pure call out of the loop and times one fib(30) as five:
   * the 2.2 ms this file reported until 2026-09-05 was exactly that. */
  volatile double argument = 30; double total = 0;
  for (int i = 0; i < 5; i++) total += unbox(fib_generic(box(argument)));
  samples[rep]=now_ms()-t0; if (rep==19) printf("%.0f\n", total); }
  for (int a=0;a<20;a++) for(int c=a+1;c<20;c++) if (samples[c]<samples[a]){double x=samples[a];samples[a]=samples[c];samples[c]=x;}
  fprintf(stderr, "workload median %.4f ms\n", samples[10]); return 0;
}
