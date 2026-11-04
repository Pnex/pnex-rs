# Why the memory runs fail (OP1_ONLY=$PWD/memory.sh ./run.sh <fc-dir>): pool policy,
# join strategy, allocator, one variable at a time on the 50M tables.
rows=50000000; Y=$D/syn$rows; O=$D/df/$rows; mkdir -p "$Y" "$O"; A=$O/a.parquet; B=$O/b.parquet
[ -f "$Y/dims.csv" ] || "$BIN/gen" "$rows" "$Y"
[ -f "$A" ] || OP1_MEM_MB=4096 "$BIN/df" convert "$Y/facts_a.csv" "$A"
[ -f "$B" ] || OP1_MEM_MB=4096 "$BIN/df" convert "$Y/facts_b.csv" "$B"
GDIFF="SELECT id, min(name) n1, max(name) n2, count(*) c, min(amount) a1, max(amount) a2 FROM (SELECT * FROM a UNION ALL SELECT * FROM b) GROUP BY id HAVING count(*) = 1 OR min(name) <> max(name) OR min(amount) <> max(amount)"
out=$O/o.parquet
caps() { # caps <stage> <sql> <tables...>
  local stage=$1 sql=$2; shift 2
  for cap in 2G 4G 8G; do rm -f "$out"; measure df "$stage" "$cap" "$out" sql "$sql" "$out" "$@" && break; done
}
export OP1_POOL=fair
caps "high/fair" "$HIGH" "f=$A"
export OP1_CONF=datafusion.optimizer.prefer_hash_join=false
caps "diff-merge/fair(1000000)" "$DIFF" "a=$A" "b=$B"
unset OP1_CONF
caps "diff-hash/fair(1000000)" "$DIFF" "a=$A" "b=$B"
caps "diff-group/fair(1000000)" "$GDIFF" "a=$A" "b=$B"
export MALLOC_ARENA_MAX=2
caps "diff-group/fair/arena2(1000000)" "$GDIFF" "a=$A" "b=$B"
unset MALLOC_ARENA_MAX
