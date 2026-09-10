#[cfg(test)]
mod tests {

    use media_decode::Thumbnailer;

    /// 依赖本地 `demo/` 样本（见 .gitignore）；CI 或无 fixture 时跳过
    #[test]
    fn it_works() {
        if !std::path::Path::new("demo/1.webp").exists() {
            eprintln!("skip thumbnial::it_works: demo/ fixtures not present");
            return;
        }
        let thumbnailer = Thumbnailer::default();

        let result = thumbnailer.create_thumbnail("demo/1.webp", "demo/output1.webp");
        assert!(result.is_ok());

        let result = thumbnailer.create_thumbnail("demo/2.png", "demo/output2.png");
        assert!(result.is_ok());

        let result = thumbnailer.create_thumbnail("demo/3.jpg", "demo/output3.jpg");
        assert!(result.is_ok());

        let result = thumbnailer.create_thumbnail("demo/4.pdf", "demo/output4.webp");
        assert!(result.is_ok());

        let result = thumbnailer.create_thumbnail("demo/5.mp4", "demo/output5.webp");
        assert!(result.is_ok());
    }
}
