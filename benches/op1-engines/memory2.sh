# Attribution of memory.sh: one variable at a time (OP1_ONLY=$PWD/memory2.sh ./run.sh <fc-dir>).
rows=50000000; O=$D/df/$rows; A=$O/a.parquet; B=$O/b.parquet; out=$O/o.parquet
GDIFF="SELECT id, min(name) n1, max(name) n2, count(*) c, min(amount) a1, max(amount) a2 FROM (SELECT * FROM a UNION ALL SELECT * FROM b) GROUP BY id HAVING count(*) = 1 OR min(name) <> max(name) OR min(amount) <> max(amount)"
MERGE=datafusion.optimizer.prefer_hash_join=false
rm -f "$out"; OP1_CONF=$MERGE measure df "diff-merge/greedy" 2G "$out" sql "$DIFF" "$out" "a=$A" "b=$B"
export OP1_POOL=fair
rm -f "$out"; OP1_CONF=$MERGE measure df "diff-merge/fair" 2G "$out" sql "$DIFF" "$out" "a=$A" "b=$B"
for w in "old_id IS NULL" "new_id IS NULL" "old_id IS NOT NULL AND new_id IS NOT NULL"; do
  echo "check [$w]: $(OP1_MEM_MB=1024 "$BIN/df" sql "SELECT * FROM t WHERE $w" "$D/.c.parquet" "t=$out")"
done
rm -f "$out"; OP1_CONF=$MERGE,datafusion.execution.spill_compression=lz4_frame measure df "diff-merge/fair/lz4" 2G "$out" sql "$DIFF" "$out" "a=$A" "b=$B"
rm -f "$out"; OP1_POOL_PCT=50 measure df "diff-group/fair/pool50" 2G "$out" sql "$GDIFF" "$out" "a=$A" "b=$B"
rm -f "$out"; OP1_POOL_PCT=25 measure df "diff-group/fair/pool25" 2G "$out" sql "$GDIFF" "$out" "a=$A" "b=$B"
