# Follow-up of run.sh (OP1_ONLY=tuning.sh ./run.sh <fc-dir>): can DataFusion hold the
# D577 snapshot diff and a high-cardinality GROUP BY under a low cap, and at what price?
O=$D/df/50000000; A=$O/a.parquet; B=$O/b.parquet
measure df fc-join 2G "$D/df/fcj.parquet" sql "$FCJOIN" "$D/df/fcj.parquet" "p=$D/df/p.parquet" "s=$D/pl/s.parquet"
measure pl fc-join-quoted 2G "$D/pl/fcj.parquet" sql "$FCJOIN" "$D/pl/fcj.parquet" "p=$D/pl/p.parquet" "s=$D/pl/s.parquet"
export OP1_CONF=datafusion.execution.sort_spill_reservation_bytes=209715200
measure df 50M-group-high-spillres 2G "$O/high2.parquet" sql "$HIGH" "$O/high2.parquet" "f=$A"
export OP1_CONF=datafusion.optimizer.prefer_hash_join=false,datafusion.execution.sort_spill_reservation_bytes=209715200
for cap in 2G 4G 8G; do
  measure df "50M-diff-merge(expect 1000000)" "$cap" "$O/diff.parquet" sql "$DIFF" "$O/diff.parquet" "a=$A" "b=$B" && break
done
export OP1_CONF=datafusion.execution.sort_spill_reservation_bytes=209715200
# Set-based diff with no join: every id appears once or twice, aggregation can spill.
GDIFF="SELECT id, min(name) n1, max(name) n2, count(*) c, min(amount) a1, max(amount) a2 FROM (SELECT * FROM a UNION ALL SELECT * FROM b) GROUP BY id HAVING count(*) = 1 OR min(name) <> max(name) OR min(amount) <> max(amount)"
for cap in 2G 4G 8G; do
  measure df "50M-diff-group(expect 1000000)" "$cap" "$O/diffg.parquet" sql "$GDIFF" "$O/diffg.parquet" "a=$A" "b=$B" && break
done
unset OP1_CONF
for cap in 4G 8G; do
  measure pl "50M-diff-group(expect 1000000)" "$cap" "$D/pl/50000000/diffg.parquet" sql "$GDIFF" "$D/pl/50000000/diffg.parquet" "a=$D/pl/50000000/a.parquet" "b=$D/pl/50000000/b.parquet" && break
done
# Diff on (key, row hash) only: the join build side no longer carries the columns.
HASH="SELECT a.id old_id, b.id new_id FROM (SELECT id, md5(concat(name, '|', cast(amount AS varchar))) h FROM a) a FULL OUTER JOIN (SELECT id, md5(concat(name, '|', cast(amount AS varchar))) h FROM b) b ON a.id = b.id WHERE a.id IS NULL OR b.id IS NULL OR a.h <> b.h"
for cap in 2G 4G 8G; do
  measure df "50M-diff-hash(expect 1000000)" "$cap" "$O/diffh.parquet" sql "$HASH" "$O/diffh.parquet" "a=$A" "b=$B" && break
done
