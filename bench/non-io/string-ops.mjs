let count = 0;
const text = "2026-09-03T20:57:34.123Z [INFO] service=payment-gateway user_id=usr_99812 action=charge_credit_card status=success amount=149.99 currency=USD latency_ms=42";
for (let i = 0; i < 50000; i++) {
  const parts = text.split(" ");
  const joined = parts.slice(2).join("&");
  const idx = joined.indexOf("amount=");
  if (idx !== -1) {
    const val = joined.substring(idx + 7, idx + 13);
    count += val.length;
  }
}
console.log(count);
