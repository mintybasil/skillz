# Inline Comments

Add an inline comment when code falls into any of these categories. If it doesn't, leave it un-commented.

## 1. Non-obvious business rule or domain logic

```
// Cap at 80% per regulation Z §226.32 — higher triggers disclosure rules.
max_loan = appraised_value * 0.80
```

## 2. Workaround, hack, or non-obvious technical decision

```
// Descending: dashboard chart assumes newest-first and doesn't re-sort.
entries.sort_by(|a, b| b.timestamp.cmp(&a.timestamp))
```

## 3. External constraint (bug report, spec, third-party quirk)

```
// Vendor API returns MM/DD/YYYY despite documenting ISO 8601. See issue #142.
let date = NaiveDate::parse_from_str(raw, "%m/%d/%Y")?;
```

## 4. Non-obvious algorithm or optimization

```
// Floyd's cycle detection: O(1) space — avoids allocating a visited set
// on the hot path.
let (slow, fast) = (head, head);
```

