# How a simulation runs

```
saldo budget.saldo --from 2026-01-01 --to 2026-12-31
```

saldo simulates one day at a time and reports every day from `--from` to
`--to`, inclusive.

## Warm-up

If an account opens before `--from`, the simulation starts on the earliest
opening date instead. Entries fire normally during this warm-up, but their
transactions aren't printed. The opening balances you see are the balances on
`--from`, after the warm-up.

Accounts without an opening balance start at zero on the first simulated day.
Period totals (`.ytd`, `.qtd`, `.mtd`) also start at zero on the first
simulated day. If that day falls partway through a period and the leg's entry
would already have fired earlier in it, saldo warns that the total is missing
amounts. To fix it, simulate from the start of the period, or open an account
by then so the warm-up covers it.

## Each day

Every simulated day runs these steps in order:

1. Period totals reset if the day starts a new month, quarter or year.
2. Params are evaluated. A param that reads an account sees the balance before
   any entries fire that day.
3. Accounts whose opening date is today get their opening balance, in
   declaration order. If any account opened, params are evaluated again so
   they can see it.
4. Entries whose schedule matches today fire, in declaration order. A later
   entry sees the balances left by earlier ones. Within one entry, every
   posting is evaluated against the balances from before that entry fired.
5. Assertions whose schedule matches today are checked against the balances
   after all entries fired.
6. Named legs from today's entries are added to their period totals. So a
   `.ytd` read during an entry doesn't include that day's amount.

The first failing assertion or evaluation error stops the simulation.

## Output

The ledger output (the default) starts with an `opening-balances` transaction
for the balances on `--from`, balanced against `Equity:OpeningBalances`. An
account that opens later gets its own `opening-balances` transaction on its
opening date. After that comes one transaction per entry firing, skipping
firings whose postings all come to zero.

The CSV output (`--format csv`) has one row per day with every account's
balance at the end of that day, in the order the accounts are declared.
