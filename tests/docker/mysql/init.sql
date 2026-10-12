CREATE DATABASE sqlshop_test CHARACTER SET utf8mb4;
CREATE DATABASE sqlshop_other CHARACTER SET utf8mb4;
CREATE TABLE sqlshop_test.navigation_probe (marker VARCHAR(100) NOT NULL);
INSERT INTO sqlshop_test.navigation_probe VALUES ('sqlshop_test');
CREATE TABLE sqlshop_other.navigation_probe (marker VARCHAR(100) NOT NULL);
INSERT INTO sqlshop_other.navigation_probe VALUES ('sqlshop_other');
