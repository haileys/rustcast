pub struct Cookies<'a>(&'a str);

impl<'a> Cookies<'a> {
    pub fn new(encoded: &'a str) -> Self {
        Cookies(encoded)
    }

    pub fn iter(&self) -> CookieIterator<'a> {
        CookieIterator(self.0)
    }

    pub fn lookup(&self, name: &str) -> Option<&'a str> {
        self.iter()
            .find(|&(k, _)| k == name)
            .map(|(_, v)| v)
    }
}

pub struct CookieIterator<'a>(&'a str);

impl<'a> Iterator for CookieIterator<'a> {
    type Item = (&'a str, &'a str);

    fn next(&mut self) -> Option<Self::Item> {
        let mut iter = self.0.splitn(2, ';');

        let cookie = iter.next().and_then(|kv| {
            let mut iter = kv.splitn(2, '=');
            iter.next().and_then(|k|
                iter.next().map(|v|
                    (k.trim_left(), v)))
        });

        self.0 = iter.next().unwrap_or("");

        cookie
    }
}
