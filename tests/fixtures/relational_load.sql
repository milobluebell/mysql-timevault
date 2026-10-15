-- Run only against a disposable MySQL instance with an empty app database.
USE app;

CREATE TABLE organizations (
  id INT PRIMARY KEY,
  name VARCHAR(100) NOT NULL
) ENGINE=InnoDB;

CREATE TABLE users (
  id INT PRIMARY KEY,
  organization_id INT NOT NULL,
  email VARCHAR(120) NOT NULL UNIQUE,
  profile_text VARCHAR(300) NOT NULL,
  FOREIGN KEY (organization_id) REFERENCES organizations(id)
) ENGINE=InnoDB;

CREATE TABLE categories (
  id INT PRIMARY KEY,
  name VARCHAR(100) NOT NULL
) ENGINE=InnoDB;

CREATE TABLE products (
  id INT PRIMARY KEY,
  category_id INT NOT NULL,
  sku VARCHAR(60) NOT NULL UNIQUE,
  description VARCHAR(300) NOT NULL,
  FOREIGN KEY (category_id) REFERENCES categories(id)
) ENGINE=InnoDB;

CREATE TABLE warehouses (
  id INT PRIMARY KEY,
  name VARCHAR(100) NOT NULL
) ENGINE=InnoDB;

CREATE TABLE inventory (
  id INT PRIMARY KEY,
  product_id INT NOT NULL,
  warehouse_id INT NOT NULL,
  quantity INT NOT NULL,
  UNIQUE KEY (product_id, warehouse_id),
  FOREIGN KEY (product_id) REFERENCES products(id),
  FOREIGN KEY (warehouse_id) REFERENCES warehouses(id)
) ENGINE=InnoDB;

CREATE TABLE orders (
  id INT PRIMARY KEY,
  user_id INT NOT NULL,
  note VARCHAR(300) NOT NULL,
  FOREIGN KEY (user_id) REFERENCES users(id)
) ENGINE=InnoDB;

CREATE TABLE order_items (
  id INT PRIMARY KEY,
  order_id INT NOT NULL,
  product_id INT NOT NULL,
  quantity INT NOT NULL,
  FOREIGN KEY (order_id) REFERENCES orders(id),
  FOREIGN KEY (product_id) REFERENCES products(id)
) ENGINE=InnoDB;

CREATE TABLE payments (
  id INT PRIMARY KEY,
  order_id INT NOT NULL,
  amount_cents INT NOT NULL,
  FOREIGN KEY (order_id) REFERENCES orders(id)
) ENGINE=InnoDB;

CREATE TABLE shipments (
  id INT PRIMARY KEY,
  order_id INT NOT NULL,
  warehouse_id INT NOT NULL,
  tracking_code VARCHAR(80) NOT NULL UNIQUE,
  FOREIGN KEY (order_id) REFERENCES orders(id),
  FOREIGN KEY (warehouse_id) REFERENCES warehouses(id)
) ENGINE=InnoDB;

CREATE TABLE digits (d INT PRIMARY KEY) ENGINE=MEMORY;
INSERT INTO digits VALUES (0),(1),(2),(3),(4),(5),(6),(7),(8),(9);
CREATE TEMPORARY TABLE seq (n INT PRIMARY KEY) ENGINE=MEMORY;
INSERT INTO seq
SELECT 1 + d0.d + 10*d1.d + 100*d2.d + 1000*d3.d
FROM digits d0 CROSS JOIN digits d1 CROSS JOIN digits d2 CROSS JOIN digits d3
WHERE d0.d + 10*d1.d + 100*d2.d + 1000*d3.d < 8000;

START TRANSACTION;
INSERT INTO organizations SELECT n, CONCAT('Org ', n) FROM seq WHERE n <= 100;
INSERT INTO categories SELECT n, CONCAT('Category ', n) FROM seq WHERE n <= 200;
INSERT INTO warehouses SELECT n, CONCAT('Warehouse ', n) FROM seq WHERE n <= 100;
INSERT INTO users SELECT n, 1 + (n - 1) % 100, CONCAT('user', n, '@example.invalid'),
  RPAD(SHA2(CONCAT('user-', n), 256), 256, 'x') FROM seq WHERE n <= 5000;
INSERT INTO products SELECT n, 1 + (n - 1) % 200, CONCAT('SKU-', n),
  RPAD(SHA2(CONCAT('product-', n), 256), 256, 'x') FROM seq WHERE n <= 4000;
INSERT INTO inventory SELECT n, 1 + (n - 1) % 4000, 1 + FLOOR((n - 1) / 4000),
  1 + n % 100 FROM seq WHERE n <= 8000;
INSERT INTO orders SELECT n, 1 + (n - 1) % 5000,
  RPAD(SHA2(CONCAT('order-', n), 256), 256, 'x') FROM seq WHERE n <= 8000;
INSERT INTO order_items SELECT (s.n - 1)*3 + d.d, s.n,
  1 + (s.n*3 + d.d - 1) % 4000, 1 + d.d
  FROM seq s JOIN digits d ON d.d BETWEEN 1 AND 3 WHERE s.n <= 8000;
INSERT INTO payments SELECT n, n, 1000 + n % 10000 FROM seq WHERE n <= 8000;
INSERT INTO shipments SELECT n, n, 1 + (n - 1) % 100,
  CONCAT('SHIP-', n) FROM seq WHERE n <= 6000;
COMMIT;

DROP TEMPORARY TABLE seq;
DROP TABLE digits;
