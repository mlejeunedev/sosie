-- Dump synthétique : cas limites du parseur mysqldump.
-- Test principal : parse → serialize SANS transformation == ce fichier, byte à byte.
-- Test secondaire : avec 02_parser_edge_cases.yaml, seule la colonne `email` change.

/*!40101 SET NAMES utf8mb4 */;

-- 1. Nom de table réservé, colonnes avec noms réservés, commentaire de colonne contenant une virgule et une parenthèse
DROP TABLE IF EXISTS `order`;
CREATE TABLE `order` (
  `id` int NOT NULL,
  `key` varchar(10) NOT NULL COMMENT 'clé (unique), attention',
  `email` varchar(180) DEFAULT NULL,
  `desc` text,
  PRIMARY KEY (`id`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4;

-- 2. Échappements : apostrophe échappée, backslash, guillemet double, saut de ligne, tabulation, NUL, chaîne vide, NULL
INSERT INTO `order` VALUES (1,'a\'b','x@y.fr','ligne1\nligne2\ttab\\backslash \"quoted\" \0nul'),(2,'','',''),(3,'c',NULL,NULL);

-- 3. INSERT sur plusieurs lignes physiques (mysqldump --skip-extended-insert) + INSERT avec liste de colonnes
INSERT INTO `order` VALUES (4,'d','d@d.fr','ok');
INSERT INTO `order` (`id`, `key`, `email`, `desc`) VALUES (5,'e','e@e.fr','avec colonnes');

-- 4. Valeurs non-chaînes : hex, _binary, nombres négatifs, flottants, dates, booléens
DROP TABLE IF EXISTS `misc`;
CREATE TABLE `misc` (
  `id` int NOT NULL,
  `blob_col` blob,
  `bin_col` varbinary(16) DEFAULT NULL,
  `f` double DEFAULT NULL,
  `d` decimal(12,4) DEFAULT NULL,
  `dt` datetime(6) DEFAULT NULL,
  `b` bit(1) DEFAULT NULL,
  `email` varchar(180) DEFAULT NULL,
  PRIMARY KEY (`id`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4;

INSERT INTO `misc` VALUES (1,0xDEADBEEF,_binary '\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0',-1.5e-10,-12345.6789,'2024-02-29 23:59:59.123456',_binary '\0','m@m.fr'),(2,NULL,NULL,NULL,NULL,NULL,NULL,NULL);

-- 5. Une valeur qui ressemble à du SQL à l'intérieur d'une chaîne (ne doit PAS être interprétée)
INSERT INTO `misc` VALUES (3,NULL,NULL,NULL,NULL,NULL,NULL,'INSERT INTO `user` VALUES (1,''x''); -- pas une vraie requête');

-- 6. Colonne générée, colonne avec DEFAULT expression, CHECK constraint, index fulltext
DROP TABLE IF EXISTS `gen`;
CREATE TABLE `gen` (
  `id` int NOT NULL,
  `email` varchar(180) NOT NULL,
  `email_domain` varchar(180) GENERATED ALWAYS AS (substring_index(`email`,_utf8mb4'@',-(1))) VIRTUAL,
  `created` datetime DEFAULT CURRENT_TIMESTAMP,
  `qty` int DEFAULT '0',
  PRIMARY KEY (`id`),
  FULLTEXT KEY `ft_email` (`email`),
  CONSTRAINT `chk_qty` CHECK ((`qty` >= 0))
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4;

-- mysqldump omet les colonnes générées dans les INSERT : il n'y a que 4 valeurs, pas 5
INSERT INTO `gen` (`id`, `email`, `created`, `qty`) VALUES (1,'g@g.fr','2024-01-01 00:00:00',3);

-- 7. Vue et trigger : transmis tels quels (Raw), avec un avertissement dans le rapport
DROP VIEW IF EXISTS `v_emails`;
/*!50001 CREATE ALGORITHM=UNDEFINED */
/*!50013 DEFINER=`app`@`%` SQL SECURITY DEFINER */
/*!50001 VIEW `v_emails` AS select `order`.`email` AS `email` from `order` */;

DELIMITER ;;
/*!50003 CREATE*/ /*!50017 DEFINER=`app`@`%`*/ /*!50003 TRIGGER `trg_order` BEFORE INSERT ON `order` FOR EACH ROW SET NEW.`key` = LOWER(NEW.`key`) */;;
DELIMITER ;

-- 8. Table sans aucune ligne (INSERT absent)
DROP TABLE IF EXISTS `empty_table`;
CREATE TABLE `empty_table` (
  `id` int NOT NULL,
  `email` varchar(180) DEFAULT NULL,
  PRIMARY KEY (`id`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4;

-- 9. Unicode, emoji, caractères 4 octets, apostrophe typographique (non échappée car ≠ ')
INSERT INTO `empty_table` VALUES (1,'émoji-😀-中文-Œuvre-l’apostrophe@x.fr');

-- Dump completed
