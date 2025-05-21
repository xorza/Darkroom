pub trait NormalizeString {
    fn normalize(&self) -> String;
}

impl NormalizeString for str {
    fn normalize(&self) -> String {
        self.replace("\r\n", "\n").replace("\r", "\n")
    }
}

impl NormalizeString for String {
    fn normalize(&self) -> String {
        self.as_str().normalize()
    }
}

impl NormalizeString for &str {
    fn normalize(&self) -> String {
        self.to_string().normalize()
    }
}
