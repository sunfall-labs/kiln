import { bench } from "../harness.mjs";

const text: string = "2026-09-03T20:57:34.123Z [INFO] service=payment-gateway user_id=usr_99812 action=charge_credit_card status=success amount=149.99 currency=USD latency_ms=42";

bench("string-ops", (): number => {
  let count: number = 0;
  for (let i: number = 0; i < 50000; i++) {
    const parts: string[] = text.split(" ");
    const joined: string = parts.slice(2).join("&");
    const idx: number = joined.indexOf("amount=");
    if (idx !== -1) {
      const val: string = joined.substring(idx + 7, idx + 13);
      count += val.length;
    }
  }
  return count;
});
