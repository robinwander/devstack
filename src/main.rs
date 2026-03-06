#[tokio::main]
async fn main() -> anyhow::Result<()> {
    devstack::run().await
}
