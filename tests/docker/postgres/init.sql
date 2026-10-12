CREATE DATABASE sqlshop_test;
CREATE DATABASE sqlshop_other;
\connect sqlshop_test
CREATE TABLE navigation_probe (marker TEXT NOT NULL);
INSERT INTO navigation_probe VALUES ('sqlshop_test');
\connect sqlshop_other
CREATE TABLE navigation_probe (marker TEXT NOT NULL);
INSERT INTO navigation_probe VALUES ('sqlshop_other');
