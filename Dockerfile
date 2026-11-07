FROM rust:1-slim AS build
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY src ./src
RUN cargo build --release

FROM debian:stable-slim
RUN useradd -r -m -d /data roamway
COPY --from=build /src/target/release/roamway /usr/local/bin/roamway
USER roamway
VOLUME ["/data"]
EXPOSE 8796
ENTRYPOINT ["roamway"]
CMD ["serve", "--bind", "0.0.0.0:8796", "--data", "/data"]
