FROM gcr.io/oss-fuzz-base/base-builder-rust
COPY . $SRC/postgres-jsonb-canonical
WORKDIR $SRC/postgres-jsonb-canonical
COPY ./.clusterfuzzlite/build.sh $SRC/
