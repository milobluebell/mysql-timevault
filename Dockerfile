FROM mysql:8.0 AS mysql80

FROM rust:1.92-alpine AS builder
WORKDIR /build
RUN apk add --no-cache musl-dev curl cpio rpm
ARG TARGETARCH
ARG MYSQL_CLIENT_VERSION=8.4.11
RUN case "$TARGETARCH" in arm64) rpm_arch=aarch64 ;; amd64) rpm_arch=x86_64 ;; *) exit 1 ;; esac \
    && curl -fsSL "https://repo.mysql.com/yum/mysql-8.4-community/el/9/${rpm_arch}/mysql-community-client-${MYSQL_CLIENT_VERSION}-1.el9.${rpm_arch}.rpm" -o /tmp/mysql-client.rpm \
    && rpm2cpio /tmp/mysql-client.rpm | cpio -id --quiet './usr/bin/mysqlbinlog' \
    && test -x /build/usr/bin/mysqlbinlog
COPY Cargo.toml Cargo.lock ./
COPY src ./src
COPY tests ./tests
RUN cargo build --release --locked

FROM mysql:8.4
COPY --from=builder /build/target/release/mysql-timevault /usr/local/bin/mysql-timevault
COPY --from=builder /build/usr/bin/mysqlbinlog /usr/local/bin/mysqlbinlog
COPY --from=mysql80 /usr/bin/mysqldump /usr/local/bin/mysqldump80
WORKDIR /app/mm-storage
ENTRYPOINT ["/usr/local/bin/mysql-timevault"]
CMD []
