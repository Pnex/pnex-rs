#!/usr/bin/env bash
# OP1.0 measurement (data-pipelines.md D572). Every engine run lives in its own
# capped scope, like a pod: usage: run.sh <fc-dir> [rows-list]
# Writes data/results.tsv and data/guard.txt. S3_* optional for the S3 stage.
set -uo pipefail
cd "$(dirname "$0")"
FC=$1; SIZES=${2:-"5000000 50000000"}
BIN=$PWD/target/release; D=$PWD/data; export TMPDIR=$D/spill OP1_DELIM=';'
mkdir -p "$D" "$TMPDIR"; R=$D/results.tsv
[ -f "$R" ] || printf 'engine\tstage\tcap\texit\twall_s\tpeak_rss_mb\tspill_peak_mb\trows\toutput_mb\n' >"$R"

# measure <engine> <stage> <cap> <out> <args...>: appends one result line.
measure() {
  local e=$1 s=$2 cap=$3 out=$4; shift 4
  local log; log=$(mktemp -p "$D"); find "${TMPDIR:?}" -mindepth 1 -delete  # spill dirs are dot dirs a killed run leaves behind
  # OP1_POOL_PCT (75) of the cap goes to the engine pool, the rest to untracked buffers.
  export OP1_MEM_MB=$(( ${cap%G} * 1024 * ${OP1_POOL_PCT:-75} / 100 ))
  ( peak=0; while :; do n=$(du -sm "$TMPDIR" | cut -f1); [ "$n" -gt "$peak" ] && { peak=$n; echo "$peak" >"$log.spill"; }; sleep 1; done ) &
  local w=$!
  systemd-run --user --scope -q -p MemoryMax="$cap" -p MemorySwapMax=0 -p CPUQuota=400% \
    timeout 1500 /usr/bin/time -f 'TIME %e %M' -o "$log.t" "$BIN/$e" "$@" >"$log" 2>&1
  local rc=$?; kill "$w" 2>/dev/null; wait "$w" 2>/dev/null
  # time(1) prepends a status line when the command fails: keep only ours.
  local wall=- rss=0; read -r _ wall rss < <(grep '^TIME' "$log.t" 2>/dev/null)
  local rows=-; [ "$rc" = 0 ] && [ -f "$out" ] && rows=$(count "$out")
  printf '%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\n' "$e" "$s" "$cap" "$rc" "$wall" $((rss/1024)) "$(cat "$log.spill" 2>/dev/null || echo 0)" "$rows" \
    "$(du -m "$out" 2>/dev/null | cut -f1)" | tee -a "$R"
  [ "$rc" = 0 ] || tail -2 "$log" | cut -c1-700
  rm -f "$log" "$log".*
  return "$rc"
}
# Row count of a Parquet file, always read by DataFusion so both outputs are checked alike.
count() { OP1_MEM_MB=1024 "$BIN/df" sql 'select * from t' "$D/.c.parquet" "t=$1" | sed -n 's/^rows=//p'; }

AGG='SELECT d.family, f.city, count(*) n, sum(f.amount) total FROM f JOIN d ON f.code = d.code WHERE d.active GROUP BY d.family, f.city'
HIGH='SELECT siret, count(*) n, sum(amount) total FROM f GROUP BY siret ORDER BY n DESC'
DIFF='SELECT a.id old_id, b.id new_id, a.name old_name, b.name new_name FROM a FULL OUTER JOIN b ON a.id = b.id WHERE a.id IS NULL OR b.id IS NULL OR a.name <> b.name OR a.amount <> b.amount'
# Quoted: DataFusion folds unquoted identifiers to lower case, Polars does not.
FCJOIN="SELECT p.\"Numero_Fiche\", p.\"Siret_Partenaire\", p.\"Habilitation_Partenaire\" FROM p JOIN s ON p.\"Numero_Fiche\" = s.\"Numero_Fiche\" WHERE s.\"Actif\" = 'ACTIVE'"

[ -n "${OP1_ONLY:-}" ] && { source "$OP1_ONLY"; exit; }

for e in df pl; do
  O=$D/$e; mkdir -p "$O"
  measure "$e" fc-convert-partners 2G "$O/p.parquet" convert "$FC"/*Partenaires*.csv "$O/p.parquet"
  measure "$e" fc-convert-standard 2G "$O/s.parquet" convert "$FC"/*Standard*.csv "$O/s.parquet"
done
# DataFusion refuses the real Standard file (see the OP1.0 report): both engines join
# the Parquet written by Polars, so the join itself stays comparable.
for e in df pl; do
  O=$D/$e
  measure "$e" fc-join 2G "$O/fcj.parquet" sql "$FCJOIN" "$O/fcj.parquet" "p=$O/p.parquet" "s=$D/pl/s.parquet"
done

for rows in $SIZES; do
  S=$D/syn$rows; mkdir -p "$S"
  [ -f "$S/dims.csv" ] || "$BIN/gen" "$rows" "$S"
  tag=$((rows/1000000))M
  for e in df pl; do
    O=$D/$e/$rows; mkdir -p "$O"
    measure "$e" "$tag-convert-a" 8G "$O/a.parquet" convert "$S/facts_a.csv" "$O/a.parquet"
    measure "$e" "$tag-convert-b" 8G "$O/b.parquet" convert "$S/facts_b.csv" "$O/b.parquet"
    measure "$e" "$tag-convert-d" 2G "$O/d.parquet" convert "$S/dims.csv" "$O/d.parquet"
    for cap in 2G 8G; do
      measure "$e" "$tag-join-agg" "$cap" "$O/agg.parquet" sql "$AGG" "$O/agg.parquet" "f=$O/a.parquet" "d=$O/d.parquet" && break
    done
    for cap in 2G 8G; do
      measure "$e" "$tag-group-high" "$cap" "$O/high.parquet" sql "$HIGH" "$O/high.parquet" "f=$O/a.parquet" && break
    done
    for cap in 2G 8G; do
      measure "$e" "$tag-diff(expect $((rows/50)))" "$cap" "$O/diff.parquet" sql "$DIFF" "$O/diff.parquet" "a=$O/a.parquet" "b=$O/b.parquet" && break
    done
  done
done

# Same query, input read from S3 (RustFS): needs S3_ENDPOINT, S3_KEY, S3_SECRET.
if [ -n "${S3_ENDPOINT:-}" ]; then
  first=${SIZES%% *}; local_a=$D/df/$first/a.parquet; remote=s3://op1-bench/a.parquet
  curl -s -o /dev/null --aws-sigv4 "aws:amz:us-east-1:s3" -u "$S3_KEY:$S3_SECRET" -X PUT "$S3_ENDPOINT/op1-bench"
  "$BIN/df" put "$local_a" "$remote"
  export AWS_ACCESS_KEY_ID=$S3_KEY AWS_SECRET_ACCESS_KEY=$S3_SECRET AWS_ENDPOINT_URL=$S3_ENDPOINT AWS_REGION=us-east-1 AWS_ALLOW_HTTP=true
  for e in df pl; do
    measure "$e" s3-group-high 2G "$D/$e/s3high.parquet" sql "$HIGH" "$D/$e/s3high.parquet" "f=$remote"
  done
fi

# Hostile SQL: must be refused, and the canary file must stay unread.
G=$D/guard.txt; : >"$G"; printf 'x\nsecret\n' >"$D/canary.csv"; rm -f "$D/escaped.csv"
printf 'a;b\n1;2\n' >"$D/t.csv"
while IFS= read -r q; do
  for e in df pl; do
    rm -f "$D/g.parquet"; res=$(OP1_MEM_MB=512 timeout 60 "$BIN/$e" sql "$q" "$D/g.parquet" "t=$D/t.csv" 2>&1); rc=$?
    printf '%s\trc=%s\t%s\t=> %s\n' "$e" "$rc" "$q" "$(echo "$res" | tail -1 | cut -c1-140)" >>"$G"
  done
done <<Q
CREATE EXTERNAL TABLE x STORED AS CSV LOCATION '$D/canary.csv'
COPY t TO '$D/escaped.csv'
COPY (SELECT * FROM t) TO '$D/escaped.csv' STORED AS CSV
INSERT INTO t VALUES (9, 9)
DROP TABLE t
SET datafusion.execution.target_partitions = 1
SELECT * FROM '$D/canary.csv'
SELECT * FROM read_csv('$D/canary.csv')
SELECT * FROM read_parquet('$D/canary.csv')
EXPLAIN SELECT * FROM t
Q
ls "$D/escaped.csv" >>"$G" 2>&1
